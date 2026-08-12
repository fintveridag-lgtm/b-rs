# 📈 Live aksjehandel med Interactive Brokers (IBKR)

Slik setter du opp **ekte** aksjehandel i b-rs. Aksjer handles KUN via
Interactive Brokers — Nordnet-koblingen er bare lesetilgang (viser
porteføljen, handler aldri), og krypto går via Revolut X.

> ⚠️ **Ta det i riktig rekkefølge:** kjør aksjer i papirmodus i noen uker
> først og følg 📊 Fasit-fanen. Går live bare hvis strategien faktisk slår
> børsen — og begynn da med små beløp.

---

## Steg 1: Opprett en IBKR-konto (tar noen dager)

1. Gå til <https://www.interactivebrokers.com> (eller ibkr.com) → **Open Account**
2. Vanlig bank-oppretting: legitimasjon (BankID/pass), KYC-spørsmål, godkjenning
3. **Overfør penger** til kontoen når den er åpnet
4. Noter **kontonummeret** ditt (starter med `U`, f.eks. `U1234567`)

Dette er en helt egen meglerkonto — uavhengig av Nordnet. Nordnet-aksjene
dine forblir hos Nordnet.

---

## Steg 2: Last ned Client Portal Gateway

Et lite gratisprogram som lar appen snakke med IBKR.

1. Logg inn på IBKR sine nettsider → søk opp **«Client Portal API»** /
   **«Client Portal Gateway»** → last ned zip-en
2. Pakk ut til en mappe du finner igjen, f.eks. `C:\ibkr-gateway`
3. Krever **Java** installert (last ned fra adoptium.net hvis du mangler det)

---

## Steg 3: Start gatewayen og logg inn

1. I gateway-mappa, kjør (Windows):
   ```
   bin\run.bat root\conf.yaml
   ```
   (La dette vinduet stå åpent — det MÅ kjøre mens du handler.)
2. Åpne **<https://localhost:5000>** i nettleseren
3. Logg inn med **IBKR-brukernavn og -passord** + to-faktor fra IBKR-appen
4. Når det står at du er innlogget/autentisert, er du klar

Nettleseren advarer om «usikkert sertifikat» — det er normalt (gatewayen
bruker et selvsignert sertifikat lokalt). Trykk deg forbi / godta.

---

## Steg 4: Koble appen til

Åpne `config.toml` (`notepad config.toml`) og sett:
```toml
mode = "live"
broker = "ibkr"
live_ok = true

[ibkr]
base_url = "https://localhost:5000/v1/api"
account = "U1234567"          # ditt kontonummer
accept_invalid_certs = true   # gatewayen bruker selvsignert sertifikat
limit_orders = true           # limit- i stedet for markedsordrer (tryggest)
limit_slippage_pct = 0.3      # hvor langt forbi siste kurs limit settes (%)
realtime_quotes = true        # sanntidskurs fra IBKR
```

Vil du handle krypto (Revolut X) OG aksjer (IBKR) samtidig, bruk i stedet
`broker = "multi"` med `[multi] crypto = "revolutx"` og `stocks = "ibkr"`.

---

## Steg 5: Markedsdata-abonnement (for sanntid)

For ekte sanntidskurs (ikke ~15 min forsinket): bestill markedsdata for
riktig børs inne på IBKRs nettkontor — for norske aksjer **Oslo Børs**
(noen få dollar/mnd). Uten dette faller appen trygt tilbake på forsinket
kurs, men da har ikke sanntids-funksjonen full effekt.

---

## Steg 6: Start appen og verifiser FØR du lar den handle

1. Start `b-rs-gui.exe` → svar **JA** på live-spørsmålet
2. Sjekk at **porteføljen og kontantsaldoen vises riktig** i appen
3. Se etter rød **LIVE**-merking
4. Begynn smått: sett `max_order_value` og `max_position_value` lavt i
   `[risk]` den første uka

---

## Daglig rutine ved live handel

- Gatewayen (`run.bat`-vinduet) må **kjøre og være innlogget** hver
  handledag — sesjonen tømmes etter noen timer / over natten, så logg inn
  på nytt på `localhost:5000` når du starter dagen
- 📱 Kill switch fra mobil (Telegram) virker som vanlig for å stoppe alt
- ⛔ Kill switch og ⏸ pause i appen fungerer også

---

## Feilsøking

| Symptom | Årsak / fiks |
|---|---|
| «fikk ikke kontakt med IBKR-gatewayen» | Gatewayen kjører ikke, eller du er ikke innlogget på `localhost:5000` |
| «IBKR-gateway svarte 401/403» | Sesjonen er utløpt — logg inn på nytt i nettleseren |
| Kursene er forsinket | Mangler markedsdata-abonnement for børsen (steg 5) |
| «fant ingen IBKR-instrument» | Symbolet finnes ikke / feil ticker — sjekk watchlisten |

---

## Kort oppsummert

- **Aksjer live:** Interactive Brokers — egen konto, gateway kjører lokalt,
  innlogging på `https://localhost:5000`
- **Nordnet:** kun visning, handler aldri
- **Krypto:** Revolut X

IBKR er litt mer fikkete enn Revolut (gateway må kjøre + være innlogget),
men har lav kurtasje og er den profesjonelle standarden.
