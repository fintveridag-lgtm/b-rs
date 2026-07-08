use crate::config::StrategyCfg;
use crate::strategy;
use crate::types::{Candle, Side};
use anyhow::Result;

/// Én avsluttet handel i backtesten.
#[derive(Debug, Clone)]
pub struct TradeRec {
    pub entry_ts: f64,
    pub exit_ts: f64,
    pub entry: f64,
    pub exit: f64,
    pub pnl_pct: f64,
}

#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub strategy: String,
    pub symbol: String,
    pub trades: Vec<TradeRec>,
    /// Åpen posisjon ved slutten av perioden (inngangskurs), om noen.
    pub open_entry: Option<f64>,
    /// Strategiens avkastning i prosent over perioden.
    pub return_pct: f64,
    /// Kjøp-og-hold-avkastning i samme periode, til sammenligning.
    pub buy_hold_pct: f64,
}

impl BacktestResult {
    pub fn wins(&self) -> usize {
        self.trades.iter().filter(|t| t.pnl_pct > 0.0).count()
    }
}

/// Kjør en strategi over historiske dagsstolper: alt-inn ved kjøpssignal,
/// alt ut ved salgssignal. Forenklet (ingen kurtasje eller glidning), men
/// nok til å sammenligne strategier og se om en idé har noe for seg.
pub fn run(
    symbol: &str,
    candles: &[Candle],
    strategy_name: &str,
    base_cfg: &StrategyCfg,
) -> Result<BacktestResult> {
    anyhow::ensure!(!candles.is_empty(), "ingen historikk å teste på");
    let mut cfg = base_cfg.clone();
    cfg.name = strategy_name.to_string();
    let mut strat = strategy::build(&cfg)?;

    const START_CASH: f64 = 100_000.0;
    let mut cash = START_CASH;
    let mut qty = 0.0_f64;
    let mut entry: Option<(f64, f64)> = None; // (ts, kurs)
    let mut trades = Vec::new();

    for c in candles {
        let Some(side) = strat.on_price(symbol, c.close) else {
            continue;
        };
        match side {
            Side::Buy if qty == 0.0 => {
                qty = cash / c.close;
                cash = 0.0;
                entry = Some((c.ts, c.close));
            }
            Side::Sell if qty > 0.0 => {
                cash = qty * c.close;
                qty = 0.0;
                if let Some((ts, price)) = entry.take() {
                    trades.push(TradeRec {
                        entry_ts: ts,
                        exit_ts: c.ts,
                        entry: price,
                        exit: c.close,
                        pnl_pct: (c.close / price - 1.0) * 100.0,
                    });
                }
            }
            _ => {}
        }
    }

    let first = candles.first().unwrap().close;
    let last = candles.last().unwrap().close;
    let final_equity = cash + qty * last;

    Ok(BacktestResult {
        strategy: strategy_name.to_string(),
        symbol: symbol.to_string(),
        trades,
        open_entry: entry.map(|(_, p)| p),
        return_pct: (final_equity / START_CASH - 1.0) * 100.0,
        buy_hold_pct: (last / first - 1.0) * 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(ts: f64, close: f64) -> Candle {
        Candle { ts, open: close, high: close, low: close, close }
    }

    #[test]
    fn sma_backtest_records_round_trip() {
        let closes = [110.0, 108.0, 106.0, 104.0, 102.0, 115.0, 125.0, 130.0, 90.0, 70.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, &c)| candle(i as f64, c))
            .collect();
        let cfg = StrategyCfg { fast: 2, slow: 4, ..StrategyCfg::default() };
        let res = run("X", &candles, "sma_cross", &cfg).unwrap();
        // Kjøp ved SMA-kryss opp (115), salg ved kryss ned (90).
        assert_eq!(res.trades.len(), 1);
        assert!(res.trades[0].pnl_pct < 0.0);
        assert!(res.open_entry.is_none());
        // Strategien tapte, men mindre enn kjøp-og-hold (110 → 70).
        assert!(res.return_pct < 0.0);
        assert!(res.return_pct > res.buy_hold_pct);
    }

    #[test]
    fn empty_history_is_an_error() {
        assert!(run("X", &[], "sma_cross", &StrategyCfg::default()).is_err());
    }
}
