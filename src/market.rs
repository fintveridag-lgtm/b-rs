//! Markedsoversikten: mest omsatte aksjer, daytrading-kandidater, populære
//! fond/ETF-er og en enkel teknisk ukesanalyse. Oppdateres av en egen
//! bakgrunnsoppgave uavhengig av handelsmotoren.

use crate::marketdata::{Snapshot, Yahoo};
use crate::state::{Flags, SharedState};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// De største/mest likvide aksjene på Oslo Børs — universet skjermene
/// regnes ut fra. Symboler som feiler hos Yahoo hoppes stille over.
pub const UNIVERSE: &[(&str, &str)] = &[
    ("EQNR.OL", "Equinor"),
    ("DNB.OL", "DNB Bank"),
    ("NHY.OL", "Norsk Hydro"),
    ("TEL.OL", "Telenor"),
    ("MOWI.OL", "Mowi"),
    ("YAR.OL", "Yara International"),
    ("AKRBP.OL", "Aker BP"),
    ("ORK.OL", "Orkla"),
    ("SALM.OL", "SalMar"),
    ("STB.OL", "Storebrand"),
    ("SUBC.OL", "Subsea 7"),
    ("TOM.OL", "Tomra Systems"),
    ("KOG.OL", "Kongsberg Gruppen"),
    ("NOD.OL", "Nordic Semiconductor"),
    ("FRO.OL", "Frontline"),
    ("GOGL.OL", "Golden Ocean"),
    ("NAS.OL", "Norwegian Air Shuttle"),
    ("SCATC.OL", "Scatec"),
    ("TGS.OL", "TGS"),
    ("AKSO.OL", "Aker Solutions"),
    ("BAKKA.OL", "Bakkafrost"),
    ("LSG.OL", "Lerøy Seafood"),
    ("MPCC.OL", "MPC Container Ships"),
    ("HAFNI.OL", "Hafnia"),
    ("VAR.OL", "Vår Energi"),
];

/// Kuraterte, folkekjære indeksfond/ETF-er med live-kurser hos Yahoo.
/// (Vanlige norske verdipapirfond har ikke sanntidskurser i åpne API-er.)
pub const FUNDS: &[(&str, &str)] = &[
    ("EUNL.DE", "iShares Core MSCI World"),
    ("VWCE.DE", "Vanguard FTSE All-World"),
    ("SXR8.DE", "iShares Core S&P 500"),
    ("IS3N.DE", "iShares Core MSCI EM"),
    ("EUNK.DE", "iShares Core MSCI Europe"),
    ("XDWT.DE", "Xtrackers MSCI World IT"),
];

#[derive(Debug, Clone)]
pub struct MarketRow {
    pub symbol: String,
    pub name: String,
    pub last: f64,
    pub day_pct: f64,
    pub week_pct: f64,
    /// Omsetning i dag: kurs × volum, i kontovaluta.
    pub turnover: f64,
    /// Snitt dagsutslag (høy−lav)/slutt siste 10 dager, i prosent.
    pub range_pct: f64,
}

#[derive(Debug, Clone)]
pub struct WeekRow {
    pub symbol: String,
    pub name: String,
    pub last: f64,
    pub week_pct: f64,
    pub rsi: f64,
    pub trend_up: bool,
    pub range_pct: f64,
    /// Sum av tekniske delsignaler, −3 … +3.
    pub score: i32,
}

#[derive(Debug, Clone, Default)]
pub struct MarketOverview {
    pub most_traded: Vec<MarketRow>,
    pub day_trade: Vec<MarketRow>,
    pub funds: Vec<MarketRow>,
    pub week: Vec<WeekRow>,
    pub updated: Option<DateTime<Utc>>,
}

fn sma(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() < n {
        return None;
    }
    Some(closes[closes.len() - n..].iter().sum::<f64>() / n as f64)
}

pub(crate) fn rsi(closes: &[f64], period: usize) -> Option<f64> {
    if closes.len() < period + 1 {
        return None;
    }
    let tail = &closes[closes.len() - period - 1..];
    let (mut gains, mut losses) = (0.0, 0.0);
    for pair in tail.windows(2) {
        let d = pair[1] - pair[0];
        if d >= 0.0 {
            gains += d;
        } else {
            losses -= d;
        }
    }
    if losses == 0.0 {
        return Some(100.0);
    }
    Some(100.0 - 100.0 / (1.0 + gains / losses))
}

fn build_row(symbol: &str, name: &str, snap: &Snapshot) -> Option<MarketRow> {
    let c = &snap.candles;
    if c.len() < 7 {
        return None;
    }
    let last = snap.last;
    let prev = c[c.len() - 2].close;
    let week_ago = c[c.len().saturating_sub(6)].close;
    let recent = &c[c.len().saturating_sub(10)..];
    let range_pct = recent
        .iter()
        .map(|b| (b.high - b.low) / b.close * 100.0)
        .sum::<f64>()
        / recent.len() as f64;
    Some(MarketRow {
        symbol: symbol.to_string(),
        name: name.to_string(),
        last,
        day_pct: (last / prev - 1.0) * 100.0,
        week_pct: (last / week_ago - 1.0) * 100.0,
        turnover: last * snap.volume,
        range_pct,
    })
}

/// Enkel teknisk vurdering av uken som kommer. IKKE investeringsråd —
/// tre delsignaler som hver bidrar −1/0/+1:
///   trend (SMA5 over/under SMA20), momentum (ukesendring ±2 %),
///   RSI (oversolgt +1 / overkjøpt −1).
pub fn analyze_week(row: &MarketRow, closes: &[f64]) -> Option<WeekRow> {
    let fast = sma(closes, 5)?;
    let slow = sma(closes, 20)?;
    let rsi = rsi(closes, 14)?;
    let trend_up = fast > slow;

    let mut score = if trend_up { 1 } else { -1 };
    if row.week_pct > 2.0 {
        score += 1;
    } else if row.week_pct < -2.0 {
        score -= 1;
    }
    if rsi < 30.0 {
        score += 1;
    } else if rsi > 70.0 {
        score -= 1;
    }

    Some(WeekRow {
        symbol: row.symbol.clone(),
        name: row.name.clone(),
        last: row.last,
        week_pct: row.week_pct,
        rsi,
        trend_up,
        range_pct: row.range_pct,
        score,
    })
}

/// Bakgrunnsoppgave: bygg markedsoversikten nå og deretter hvert 10. minutt.
pub async fn task(state: SharedState, flags: Arc<Flags>) {
    let yahoo = match Yahoo::new() {
        Ok(y) => y,
        Err(e) => {
            state.lock().unwrap().log(format!("Markedsoversikt feilet ved oppstart: {e:#}"));
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(600));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while !flags.quit.load(Ordering::Relaxed) {
        interval.tick().await;

        let mut stocks = Vec::new();
        let mut week = Vec::new();
        for (symbol, name) in UNIVERSE {
            let Ok(snap) = yahoo.snapshot(symbol).await else { continue };
            let Some(row) = build_row(symbol, name, &snap) else { continue };
            let closes: Vec<f64> = snap.candles.iter().map(|c| c.close).collect();
            if let Some(w) = analyze_week(&row, &closes) {
                week.push(w);
            }
            stocks.push(row);
        }

        let mut funds = Vec::new();
        for (symbol, name) in FUNDS {
            let Ok(snap) = yahoo.snapshot(symbol).await else { continue };
            if let Some(row) = build_row(symbol, name, &snap) {
                funds.push(row);
            }
        }

        // Mest omsatte: ren omsetning i dag.
        let mut most_traded = stocks.clone();
        most_traded.sort_by(|a, b| b.turnover.total_cmp(&a.turnover));
        most_traded.truncate(10);

        // Daytrading-kandidater: høyest dagsutslag blant de likvide
        // (over median omsetning) — bevegelse uten likviditet er en felle.
        let mut turnovers: Vec<f64> = stocks.iter().map(|r| r.turnover).collect();
        turnovers.sort_by(f64::total_cmp);
        let median = turnovers.get(turnovers.len() / 2).copied().unwrap_or(0.0);
        let mut day_trade: Vec<MarketRow> = stocks
            .iter()
            .filter(|r| r.turnover >= median)
            .cloned()
            .collect();
        day_trade.sort_by(|a, b| b.range_pct.total_cmp(&a.range_pct));
        day_trade.truncate(10);

        week.sort_by(|a, b| b.score.cmp(&a.score).then(b.week_pct.total_cmp(&a.week_pct)));

        let count = stocks.len();
        {
            let mut st = state.lock().unwrap();
            st.market = MarketOverview {
                most_traded,
                day_trade,
                funds,
                week,
                updated: Some(Utc::now()),
            };
            st.log(format!("Markedsoversikt oppdatert ({count} aksjer analysert)."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Candle;

    fn snap(closes: &[f64], volume: f64) -> Snapshot {
        let candles = closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Candle {
                ts: i as f64,
                open: c,
                high: c * 1.02,
                low: c * 0.98,
                close: c,
            })
            .collect();
        Snapshot { last: *closes.last().unwrap(), volume, candles }
    }

    #[test]
    fn builds_row_with_turnover_and_ranges() {
        let s = snap(&[100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0, 108.0], 1_000_000.0);
        let row = build_row("X.OL", "X", &s).unwrap();
        assert!((row.turnover - 108.0 * 1_000_000.0).abs() < 1.0);
        assert!(row.day_pct > 0.0);
        assert!(row.week_pct > 0.0);
        assert!(row.range_pct > 3.0 && row.range_pct < 5.0); // (high-low)/close = 4 %
    }

    #[test]
    fn week_analysis_scores_uptrend_positive() {
        // 25 dager jevn oppgang: trend opp, sterk uke, RSI høy (overkjøpt −1).
        let closes: Vec<f64> = (0..25).map(|i| 100.0 + i as f64).collect();
        let s = snap(&closes, 1_000.0);
        let row = build_row("X.OL", "X", &s).unwrap();
        let w = analyze_week(&row, &closes).unwrap();
        assert!(w.trend_up);
        assert_eq!(w.score, 1); // +1 trend, +1 momentum, −1 overkjøpt
    }
}
