//! Import av transaksjonshistorikk fra Nordnet-CSV — så den EKTE porteføljen
//! (snittkurser, realisert gevinst, skattegrunnlag) kommer inn i appen.
//!
//! Nordnet eksporterer «Transaksjoner og notater» som CSV, historisk i
//! UTF-16LE med tabulator som skilletegn — men vi tåler også UTF-8 og
//! semikolon. Kolonner gjenkjennes på navn, ikke posisjon.

use anyhow::{Context, Result};

/// Én handel fra CSV-filen, klar til å lagres som fylt ordre.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrade {
    /// Stabil id ("NN-<Nordnet-id>") — duplikatvern ved gjentatt import.
    pub id: String,
    pub ts_rfc3339: String,
    /// Verdipapirnavnet slik Nordnet skriver det (f.eks. "EQUINOR").
    pub symbol: String,
    pub is_buy: bool,
    pub qty: f64,
    pub price: f64,
}

/// Tolk en Nordnet-CSV. Ukjente transaksjonstyper (utbytte, innskudd,
/// gebyrer) hoppes stille over — vi henter kun kjøp og salg.
pub fn parse_nordnet_csv(bytes: &[u8]) -> Result<Vec<ImportedTrade>> {
    let text = decode(bytes);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().context("tom fil")?;
    let sep = if header.contains('\t') { '\t' } else { ';' };

    let cols: Vec<String> = header.split(sep).map(|c| c.trim().trim_matches('"').to_lowercase()).collect();
    let idx = |names: &[&str]| -> Option<usize> {
        names.iter().find_map(|n| cols.iter().position(|c| c == n))
    };
    let i_type = idx(&["transaksjonstype"]).context("fant ikke kolonnen Transaksjonstype — er dette en Nordnet-eksport?")?;
    let i_paper = idx(&["verdipapir"]).context("fant ikke kolonnen Verdipapir")?;
    let i_qty = idx(&["antall"]).context("fant ikke kolonnen Antall")?;
    let i_price = idx(&["kurs"]).context("fant ikke kolonnen Kurs")?;
    let i_date = idx(&["handelsdag", "bokføringsdag"]).context("fant ikke Handelsdag/Bokføringsdag")?;
    let i_id = idx(&["id", "transaksjons-id", "verifikationsnummer"]);

    let mut out = Vec::new();
    for (line_no, line) in lines.enumerate() {
        let fields: Vec<&str> = line.split(sep).collect();
        let get = |i: usize| fields.get(i).map(|s| s.trim().trim_matches('"')).unwrap_or("");

        let ttype = get(i_type).to_uppercase();
        let is_buy = ttype.starts_with("KJØP") || ttype.starts_with("KJOP");
        let is_sell = ttype.starts_with("SOLGT") || ttype.starts_with("SALG");
        if !is_buy && !is_sell {
            continue;
        }

        let symbol = get(i_paper).to_string();
        let qty = parse_norwegian_number(get(i_qty)).abs();
        let price = parse_norwegian_number(get(i_price));
        if symbol.is_empty() || qty <= 0.0 || price <= 0.0 {
            continue;
        }

        let date = get(i_date);
        let ts_rfc3339 = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .or_else(|_| chrono::NaiveDate::parse_from_str(date, "%d.%m.%Y"))
            .ok()
            .and_then(|d| d.and_hms_opt(12, 0, 0))
            .map(|dt| dt.and_utc().to_rfc3339())
            .with_context(|| format!("ugyldig dato «{date}» på linje {}", line_no + 2))?;

        let id = match i_id.map(get).filter(|s| !s.is_empty()) {
            Some(nid) => format!("NN-{nid}"),
            // Uten id-kolonne: deterministisk nøkkel av innholdet.
            None => format!("NN-{date}-{symbol}-{ttype}-{qty}-{price}"),
        };

        out.push(ImportedTrade { id, ts_rfc3339, symbol, is_buy, qty, price });
    }
    // Eldst først, som resten av ordreloggen — uansett hvilken vei
    // filen var sortert (Nordnet eksporterer nyest først).
    out.sort_by(|a, b| a.ts_rfc3339.cmp(&b.ts_rfc3339));
    Ok(out)
}

/// Nordnet-tall: "1 234,56" (gjerne med hardt mellomrom) → 1234.56.
fn parse_norwegian_number(s: &str) -> f64 {
    s.replace(['\u{a0}', ' '], "")
        .replace(',', ".")
        .parse()
        .unwrap_or(0.0)
}

/// UTF-16 (LE/BE, med BOM) eller UTF-8 — Nordnet har brukt begge.
fn decode(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semicolon_utf8() {
        let csv = "Id;Handelsdag;Transaksjonstype;Verdipapir;Antall;Kurs;Beløp\n\
                   1001;2026-05-12;KJØPT;EQUINOR;10;342,50;-3425\n\
                   1002;2026-06-01;SOLGT;EQUINOR;5;360,00;1800\n\
                   1003;2026-06-02;UTBYTTE;EQUINOR;0;0;120";
        let trades = parse_nordnet_csv(csv.as_bytes()).unwrap();
        assert_eq!(trades.len(), 2, "utbytte skal hoppes over");
        // Eldst først.
        assert!(trades[0].is_buy);
        assert_eq!(trades[0].id, "NN-1001");
        assert_eq!(trades[0].qty, 10.0);
        assert_eq!(trades[0].price, 342.5);
        assert!(!trades[1].is_buy);
    }

    #[test]
    fn parses_tab_separated_utf16le() {
        let csv = "Id\tHandelsdag\tTransaksjonstype\tVerdipapir\tAntall\tKurs\n\
                   7\t2025-11-03\tKJØPT\tMOWI\t1 200\t198,40";
        // Kod om til UTF-16LE med BOM, slik Nordnet gjør.
        let mut bytes = vec![0xFF, 0xFE];
        for u in csv.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let trades = parse_nordnet_csv(&bytes).unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].symbol, "MOWI");
        assert_eq!(trades[0].qty, 1_200.0);
        assert_eq!(trades[0].price, 198.4);
    }

    #[test]
    fn norwegian_numbers() {
        assert_eq!(parse_norwegian_number("1 234,56"), 1234.56);
        assert_eq!(parse_norwegian_number("342,50"), 342.5);
        assert_eq!(parse_norwegian_number("tull"), 0.0);
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(parse_nordnet_csv(b"helt;feil;fil\n1;2;3").is_err());
    }
}
