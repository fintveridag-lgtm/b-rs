# Dele b-tipping med andre

Slik kan du dele Norsk Tipping-analysen med venner. Det finnes to måter —
den enkle (send selve programfilen) og den skikkelige (de bygger den selv).

---

## Måte 1: Send programfilen (enklest — for venner uten teknisk erfaring)

Programmet er én enkelt fil som ikke krever installasjon. Du bygger den én
gang og kan sende den til hvem du vil.

**Slik lager du fila (på din PC):**

1. Åpne en terminal i `b-rs`-mappen (skriv `cmd` i adressefeltet i
   Filutforsker og trykk Enter).
2. Kjør:
   ```bash
   cargo build --release
   ```
3. Ferdig fil ligger nå i `b-rs\target\release\b-tipping-gui.exe`.

**Slik deler du den:**

- Send `b-tipping-gui.exe` via e-post, Dropbox, USB-minnepinne e.l.
- Mottakeren lagrer fila der de vil og **dobbeltklikker** den — ingen
  installasjon, ingen Rust, ingenting annet trengs.

**To ting mottakeren bør vite:**

- **Windows kan vise en advarsel** («Windows beskyttet PC-en din») fordi
  fila ikke er signert av en kjent utgiver. Det er normalt for hjemmelagde
  programmer — de trykker **«Mer info» → «Kjør likevel»**.
- Fila virker bare på **samme type PC** som du bygde den på (en
  Windows-fil kjører ikke på Mac, og omvendt). Skal du dele med en
  Mac-bruker, må fila bygges på en Mac.

---

## Måte 2: De bygger den selv fra kildekoden (for de litt mer teknisk anlagte)

Da får de alltid siste versjon og kan bygge både skrivebords-appen og
terminalversjonen selv.

**Det de trenger å installere én gang:**

- **Rust:** last ned fra [rustup.rs](https://rustup.rs) (på Windows: kjør
  `rustup-init.exe`, velg standardvalg).
- **Git:** [git-scm.com](https://git-scm.com/download/win) (Windows).

**Så henter og kjører de appen:**

```bash
git clone <lenken-til-dette-repoet>
cd b-rs
cargo run --release --bin b-tipping-gui
```

Første bygging tar noen minutter; etterpå starter den på sekunder.

---

## Hva appen gjør

- **b-tipping-gui** — vindusapp med faner for Lotto, Vikinglotto og
  Eurojackpot: hent trekningshistorikk, se frekvensstatistikk, gjenganger-
  analyse og de 10 rekkene med lavest forventet premiedeling.
- **b-tipping** — samme analyse i terminalen (`hent`, `analyse`, `sonde`,
  `jakt`).

Hver person som kjører appen laster ned sin egen historikk (lagres
permanent i `%APPDATA%\b-tipping` på Windows). Historikken bygges opp over
tid for hver gang de trykker «Hent historikk».

---

## Ærlig påminnelse (den gjelder alle du deler med)

Appen gir **ikke** vinnertips. Alle rekker har nøyaktig samme vinnersjanse,
historikk kan ikke forutsi neste trekning, og forventet tap er ~50 kr per
100 kr spilt. Dette er et verktøy for innsikt og underholdning — ikke en
måte å tjene penger på. Ta det med når du deler.

Sett grenser hos Norsk Tipping. Tar spillingen overhånd: Hjelpelinjen
**800 800 40** (gratis og anonymt).
