use crate::config::{BacktestCfg, StrategyCfg};
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
    /// Strategiens avkastning i prosent over perioden, etter kostnader.
    pub return_pct: f64,
    /// Kjøp-og-hold-avkastning i samme periode, til sammenligning.
    pub buy_hold_pct: f64,
    /// Verste fall fra topp underveis (negativt tall, i prosent).
    pub max_drawdown_pct: f64,
    /// Sum kurtasje og glidning betalt, i kontovaluta.
    pub costs_paid: f64,
    /// Egenkapital gjennom perioden — for resultatgrafen.
    pub equity_curve: Vec<[f64; 2]>,
}

impl BacktestResult {
    pub fn wins(&self) -> usize {
        self.trades.iter().filter(|t| t.pnl_pct > 0.0).count()
    }
}

/// Kjør en strategi over historiske dagsstolper: alt-inn ved kjøpssignal,
/// alt ut ved salgssignal — med kurtasje og glidning fra konfigen, så
/// tallene ligner virkeligheten. Fortsatt en forenkling (daglige
/// sluttkurser, full likviditet), så små marginer bør ikke stoles på.
pub fn run(
    symbol: &str,
    candles: &[Candle],
    strategy_name: &str,
    base_cfg: &StrategyCfg,
    costs: &BacktestCfg,
) -> Result<BacktestResult> {
    anyhow::ensure!(!candles.is_empty(), "ingen historikk å teste på");
    let mut cfg = base_cfg.clone();
    cfg.name = strategy_name.to_string();
    let mut strat = strategy::build(&cfg)?;

    const START_CASH: f64 = 100_000.0;
    let fee = costs.commission_pct / 100.0;
    let slip = costs.slippage_pct / 100.0;

    let mut cash = START_CASH;
    let mut qty = 0.0_f64;
    let mut entry: Option<(f64, f64)> = None; // (ts, effektiv kurs)
    let mut trades = Vec::new();
    let mut costs_paid = 0.0;
    let mut equity_curve = Vec::with_capacity(candles.len());
    let mut peak = START_CASH;
    let mut max_drawdown_pct = 0.0_f64;

    for c in candles {
        if let Some(side) = strat.on_price(symbol, c.close) {
            match side {
                Side::Buy if qty == 0.0 => {
                    // Glidning: du får litt dårligere kurs enn sluttkursen.
                    let exec = c.close * (1.0 + slip);
                    qty = cash / (exec * (1.0 + fee));
                    let fees = qty * exec * fee;
                    costs_paid += fees + qty * (exec - c.close);
                    cash = 0.0;
                    entry = Some((c.ts, exec));
                }
                Side::Sell if qty > 0.0 => {
                    let exec = c.close * (1.0 - slip);
                    let proceeds = qty * exec;
                    let fees = proceeds * fee;
                    costs_paid += fees + qty * (c.close - exec);
                    cash = proceeds - fees;
                    if let Some((ts, price)) = entry.take() {
                        trades.push(TradeRec {
                            entry_ts: ts,
                            exit_ts: c.ts,
                            entry: price,
                            exit: exec,
                            pnl_pct: (exec / price - 1.0) * 100.0,
                        });
                    }
                    qty = 0.0;
                }
                _ => {}
            }
        }

        let equity = cash + qty * c.close;
        equity_curve.push([c.ts, equity]);
        peak = peak.max(equity);
        if peak > 0.0 {
            max_drawdown_pct = max_drawdown_pct.min((equity / peak - 1.0) * 100.0);
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
        max_drawdown_pct,
        costs_paid,
        equity_curve,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(ts: f64, close: f64) -> Candle {
        Candle { ts, open: close, high: close, low: close, close }
    }

    fn candles(closes: &[f64]) -> Vec<Candle> {
        closes.iter().enumerate().map(|(i, &c)| candle(i as f64, c)).collect()
    }

    fn no_costs() -> BacktestCfg {
        BacktestCfg { commission_pct: 0.0, slippage_pct: 0.0 }
    }

    #[test]
    fn sma_backtest_records_round_trip() {
        let cs = candles(&[110.0, 108.0, 106.0, 104.0, 102.0, 115.0, 125.0, 130.0, 90.0, 70.0]);
        let cfg = StrategyCfg { fast: 2, slow: 4, ..StrategyCfg::default() };
        let res = run("X", &cs, "sma_cross", &cfg, &no_costs()).unwrap();
        // Kjøp ved SMA-kryss opp (115), salg ved kryss ned (90).
        assert_eq!(res.trades.len(), 1);
        assert!(res.trades[0].pnl_pct < 0.0);
        assert!(res.open_entry.is_none());
        // Strategien tapte, men mindre enn kjøp-og-hold (110 → 70).
        assert!(res.return_pct < 0.0);
        assert!(res.return_pct > res.buy_hold_pct);
        // Drawdown er negativ og minst like dyp som sluttavkastningen.
        assert!(res.max_drawdown_pct < 0.0);
        assert!(res.max_drawdown_pct <= res.return_pct);
        assert_eq!(res.equity_curve.len(), cs.len());
    }

    #[test]
    fn costs_reduce_returns() {
        let cs = candles(&[110.0, 108.0, 106.0, 104.0, 102.0, 115.0, 125.0, 130.0, 90.0, 70.0]);
        let cfg = StrategyCfg { fast: 2, slow: 4, ..StrategyCfg::default() };
        let free = run("X", &cs, "sma_cross", &cfg, &no_costs()).unwrap();
        let costly = run(
            "X",
            &cs,
            "sma_cross",
            &cfg,
            &BacktestCfg { commission_pct: 0.5, slippage_pct: 0.2 },
        )
        .unwrap();
        assert!(costly.return_pct < free.return_pct);
        assert!(costly.costs_paid > 0.0);
        assert_eq!(free.costs_paid, 0.0);
    }

    #[test]
    fn empty_history_is_an_error() {
        assert!(run("X", &[], "sma_cross", &StrategyCfg::default(), &no_costs()).is_err());
    }
}
