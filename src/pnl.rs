//! Realisert gevinst/tap etter FIFO-prinsippet (først inn, først ut) —
//! samme metode som Skatteetaten forventer for aksjer. Beregnes fra de
//! registrerte fylte ordrene i databasen.

use anyhow::Result;
use chrono::DateTime;
use std::collections::{HashMap, VecDeque};
use std::path::Path;

/// Én fylt ordre, kronologisk input til beregningen.
#[derive(Debug, Clone)]
pub struct Fill {
    pub ts_rfc3339: String,
    pub symbol: String,
    pub is_buy: bool,
    pub qty: f64,
    pub price: f64,
}

/// Ett realisert (del)salg: én kjøpspost (lot) konsumert av et salg.
#[derive(Debug, Clone)]
pub struct RealizedTrade {
    pub date: String,
    pub year: i32,
    pub symbol: String,
    pub qty: f64,
    pub buy_price: f64,
    pub sell_price: f64,
    pub gain: f64,
}

/// FIFO: hvert salg konsumerer de eldste kjøpspostene først.
pub fn realized_fifo(fills: &[Fill]) -> Vec<RealizedTrade> {
    let mut lots: HashMap<String, VecDeque<(f64, f64)>> = HashMap::new(); // (qty, pris)
    let mut out = Vec::new();

    for f in fills {
        if f.is_buy {
            lots.entry(f.symbol.clone()).or_default().push_back((f.qty, f.price));
            continue;
        }
        let (date, year) = match DateTime::parse_from_rfc3339(&f.ts_rfc3339) {
            Ok(dt) => (dt.format("%d.%m.%Y").to_string(), dt.format("%Y").to_string().parse().unwrap_or(0)),
            Err(_) => (f.ts_rfc3339.clone(), 0),
        };
        let queue = lots.entry(f.symbol.clone()).or_default();
        let mut remaining = f.qty;
        while remaining > 1e-9 {
            let Some((lot_qty, lot_price)) = queue.front_mut() else {
                // Salg uten registrert kjøp (f.eks. eldre historikk) — hopp over resten.
                break;
            };
            let take = remaining.min(*lot_qty);
            out.push(RealizedTrade {
                date: date.clone(),
                year,
                symbol: f.symbol.clone(),
                qty: take,
                buy_price: *lot_price,
                sell_price: f.price,
                gain: (f.price - *lot_price) * take,
            });
            *lot_qty -= take;
            remaining -= take;
            if *lot_qty <= 1e-9 {
                queue.pop_front();
            }
        }
    }
    out
}

/// Sum gevinst for et gitt år (0 = alle år).
pub fn total_gain(trades: &[RealizedTrade], year: i32) -> f64 {
    trades
        .iter()
        .filter(|t| year == 0 || t.year == year)
        .map(|t| t.gain)
        .sum()
}

/// Skriv skatterapport som CSV (semikolon-separert, norsk Excel-vennlig).
pub fn export_realized_csv(trades: &[RealizedTrade], path: &Path) -> Result<()> {
    let mut out = String::from("År;Salgsdato;Symbol;Antall;Kjøpskurs;Salgskurs;Gevinst\n");
    for t in trades {
        out.push_str(&format!(
            "{};{};{};{:.4};{:.4};{:.4};{:.2}\n",
            t.year, t.date, t.symbol, t.qty, t.buy_price, t.sell_price, t.gain
        ));
    }
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buy(ts: &str, sym: &str, qty: f64, price: f64) -> Fill {
        Fill { ts_rfc3339: ts.into(), symbol: sym.into(), is_buy: true, qty, price }
    }
    fn sell(ts: &str, sym: &str, qty: f64, price: f64) -> Fill {
        Fill { ts_rfc3339: ts.into(), symbol: sym.into(), is_buy: false, qty, price }
    }

    #[test]
    fn simple_round_trip() {
        let fills = [
            buy("2026-01-05T10:00:00+00:00", "EQNR.OL", 10.0, 100.0),
            sell("2026-02-05T10:00:00+00:00", "EQNR.OL", 10.0, 120.0),
        ];
        let trades = realized_fifo(&fills);
        assert_eq!(trades.len(), 1);
        assert!((trades[0].gain - 200.0).abs() < 1e-9);
        assert_eq!(trades[0].year, 2026);
        assert!((total_gain(&trades, 2026) - 200.0).abs() < 1e-9);
        assert_eq!(total_gain(&trades, 2025), 0.0);
    }

    #[test]
    fn fifo_consumes_oldest_lots_first() {
        let fills = [
            buy("2026-01-05T10:00:00+00:00", "X", 10.0, 100.0),
            buy("2026-01-10T10:00:00+00:00", "X", 10.0, 110.0),
            sell("2026-03-01T10:00:00+00:00", "X", 15.0, 120.0),
        ];
        let trades = realized_fifo(&fills);
        // 10 stk fra 100-loten (+200) og 5 stk fra 110-loten (+50).
        assert_eq!(trades.len(), 2);
        assert!((trades[0].gain - 200.0).abs() < 1e-9);
        assert!((trades[1].gain - 50.0).abs() < 1e-9);
        assert!((total_gain(&trades, 0) - 250.0).abs() < 1e-9);
    }

    #[test]
    fn sell_without_buy_is_skipped() {
        let fills = [sell("2026-01-05T10:00:00+00:00", "X", 5.0, 100.0)];
        assert!(realized_fifo(&fills).is_empty());
    }
}
