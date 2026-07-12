<img src="assets/logo.png" align="right" width="96" alt="b-rs-logo">

# b-rs — børs-konsoll

Megler-agnostisk handelskonsoll i Rust: følger aksjer på Oslo Børs (og andre
markeder), evaluerer en strategi fortløpende og kjøper/selger via en
utskiftbar megler-adapter. Starter alltid i **papirhandel** (simulering).

```
┌ b-rs ─ PAPIR ─ megler: paper ─ AKTIV ─ kontanter/egenkapital/P&L ────────┐
├ Watchlist (sanntidskurser) ───────────┬ Posisjoner (bot + [NN] Nordnet) ─┤
├ Ordrer ──────────────────────────────────────────────────────────────────┤
└ Hendelseslogg ───────────────────────────────────────────────────────────┘
```

## Kom i gang

```bash
cp config.example.toml config.toml   # tilpass watchlist, strategi, risiko
cargo run --release
```

Dette åpner den **grafiske appen**: watchlist, interaktiv kursgraf med
linje- eller candlestick-visning og strategiens SMA-linjer, strategivelger
med backtesting, hurtighandel, posisjoner, ordrer, hendelseslogg og knapper
for kill switch og pause.

Det bygges to programfiler:

- `b-rs` — med konsollvindu bak (viser feilmeldinger, støtter `--tui` og
  JA-bekreftelsen for live-handel)
- `b-rs-gui` — **ren vindusapp uten konsoll**, den du vil ha på skrivebordet.
  Av sikkerhetsgrunner krever den `live_ok = true` i konfigen for live-handel;
  ellers kjører den papirmodus.

Vil du heller ha terminalversjonen:

```bash
cargo run --release -- --tui
```

Taster i terminalversjonen: `q` avslutt · `k` kill switch (kanseller + stopp handel) · `p` pause strategi.

## Arkitektur

| Modul | Ansvar |
|---|---|
| `broker/` | `Broker`-traiten + implementasjoner: `paper` (simulering), `ibkr` (aksjer, Interactive Brokers) og `revolutx` (krypto, Revolut X med Ed25519-signert REST) |
| `marketdata` | Kurser og historikk fra Yahoo Finance (gratis, ~15 min forsinket; `.OL`-suffiks for Oslo Børs) |
| `market` | Markedsskjermene: mest omsatte, daytrading-kandidater, fond/ETF-er og teknisk ukesanalyse |
| `morgan` | 🧠 AI-analysesjefen: komplett screeningrapport (topp 10, P/E, gjeld, utbytte, kursmål, stop-loss) og dypdykk per aksje via Claude — krever `ANTHROPIC_API_KEY` |

Automatikk i motoren: **limit-ordrer** («kjøp hvis kursen faller til X»),
**spareavtaler** (fast kronebeløp samme dag hver måned), **ukesrapport** til
mobilen fredag ettermiddag, og **sparemål** med fremdriftslinje i porteføljen.
Nyheter per aksje vises under grafen (Yahoo Finance).
| `strategy` | `Strategy`-traiten + `sma_cross`, `rsi` og `momentum` — byttes i appen |
| `backtest` | Kjør en strategi over historikken og sammenlign med kjøp-og-hold |
| `risk` | Harde grenser: maks ordreverdi, maks posisjon, ratebegrensning, tapsgrense — pluss stop-loss/take-profit/trailing stop per posisjon |
| `pnl` | Realisert gevinst/tap (FIFO) og skatterapport-eksport (CSV) |
| `engine` | Hovedløkken: kurser → strategi → risikosjekk → ordre → tilstand |
| `nordnet` | **Lesemodus** mot Nordnets uoffisielle web-API (kun portefølje, aldri handel) |
| `notify` | Push-varsler til mobil via ntfy.sh eller Telegram (ordrer, kill switch, tapsgrense) |
| `store` | SQLite-logg over alle ordrer og hendelser (feilsøking + skattegrunnlag) |
| `gui` | egui-vindusapp med kursgraf (standard) |
| `ui` | ratatui-terminalgrensesnitt (`--tui`) |

Ny megler = ny implementasjon av `Broker`-traiten i `src/broker/` — resten av
appen er uendret. Det er slik Nordnet kan kobles på den dagen de åpner sitt
offisielle API igjen.

## Live-handel med Interactive Brokers

1. Last ned og start [Client Portal Gateway](https://www.interactivebrokers.com/en/trading/ib-api.php), logg inn på `https://localhost:5000`.
2. Fyll ut `[ibkr]`-seksjonen i `config.toml` med kontonummeret ditt.
3. Sett `mode = "live"` og `broker = "ibkr"`.
4. Appen krever at du skriver `JA` ved oppstart før den sender ekte ordrer.

**Test alltid strategien grundig i papirmodus først.** Risikogrensene i
`[risk]` er siste skanse, ikke strategi.

## Krypto via Revolut X

Revolut har ikke API for aksjehandel på personkontoer, men kryptobørsen
**Revolut X** har et offisielt REST-API som boten støtter:

1. Lag et Ed25519-nøkkelpar: `openssl genpkey -algorithm ed25519 -out revolutx.pem`
2. Registrer den offentlige delen (`openssl pkey -in revolutx.pem -pubout`)
   i Revolut X → Settings → API keys, og sett API-nøkkelen i miljøvariabelen
   `REVOLUTX_API_KEY`.
3. I `config.toml`: `broker = "revolutx"`, fyll ut `[revolutx]`-seksjonen,
   og bruk kryptosymboler i watchlisten (`BTC-USD`, `ETH-USD`, …) — samme
   format fungerer for kursdata og handel.

Kryptomarkedet er åpent hele døgnet, så papirmodus kan testes når som helst.
Merk: Revolut X oppgir ikke kostpris via API-et, så «urealisert» vises som 0
for Revolut X-posisjoner.

## Nordnet-lesemodus

Setter du `nordnet.enabled = true` og miljøvariablene `NORDNET_USERNAME` /
`NORDNET_PASSWORD`, henter appen porteføljen din fra Nordnet og viser den i
posisjonspanelet merket `[NN]`.

⚠️ Dette bruker Nordnets **uoffisielle** web-API: det kan bryte vilkårene
deres, slutte å virke uten varsel, og fungerer ikke med BankID-pålogging.
Modulen **leser kun** — den kan ikke legge ordrer hos Nordnet.

## Ansvarsfraskrivelse

Dette er et hobbyverktøy, ikke investeringsrådgivning. Automatisert handel
kan gi raske tap. Du er selv ansvarlig for ordrene boten sender, for skatt
(IBKR rapporterer ikke automatisk til Skatteetaten slik Nordnet gjør), og
for at bruken din følger meglernes vilkår.
