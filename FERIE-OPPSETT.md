# 💻 Sette opp b-rs på en ny PC (ferie-guide)

Komplett sjekkliste for å få hele appen inn på en ny Windows-PC.
Regn med ca. 30–45 minutter (mest nedlasting og venting).

---

## A) Installer verktøyene (engangsjobb)

1. **Git** — for å hente koden.
   Last ned fra <https://git-scm.com> → installer med standardvalg.

2. **Visual Studio C++ Build Tools** — Rust trenger disse på Windows.
   Gå til <https://visualstudio.microsoft.com/downloads> → «Tools for Visual
   Studio» → **Build Tools for Visual Studio**. I installasjonen: huk av
   **«Desktop development with C++»** → installer.

3. **Rust** — språket appen er skrevet i.
   Gå til <https://rustup.rs> → last ned `rustup-init.exe` → kjør →
   trykk **1** (standard) → Enter. Lukk og åpne ledetekst på nytt etterpå.

4. **(Valgfritt, anbefalt) Ollama** — gratis AI til Morgan/Stanley/Uno-X.
   Last ned fra <https://ollama.com> → installer. Åpne ledetekst og kjør:
   ```
   ollama pull llama3.1:8b
   ```

---

## B) Hent koden

Åpne ledetekst (cmd) og kjør:
```
cd %USERPROFILE%\Documents
git clone https://github.com/fintveridag-lgtm/b-rs.git
cd b-rs
git checkout claude/hei-y0oa5l
```
Første gang ber Git deg logge inn på GitHub — bruk samme konto som eier repoet.

---

## C) Bygg og kjør

```
cargo build --release
copy config.example.toml config.toml
.\target\release\b-rs-gui.exe
```
Nå starter appen i **papirmodus** (lekepenger) — trygt å utforske. 🎉

---

## D) Slå på AI (gratis, via Ollama)

I appen: ⚙ Innstillinger → Morgan → Hjerne = **ollama**.
Da virker Morgan, Stanley og Uno-X uten nøkler og uten kostnad.

---

## E) Valgfritt: Claude og/eller live handel

Nøklene ligger bare på hjemme-PC-en, så de må settes på nytt her.
I ledetekst (start appen på nytt etterpå):
```
setx ANTHROPIC_API_KEY "sk-ant-din-nøkkel"
setx TELEGRAM_BOT_TOKEN "din-telegram-token"
setx REVOLUTX_API_KEY "din-revolut-nøkkel"
```

For **live Revolut-handel** trenger du i tillegg:
- Nøkkelfilen `revolutx.pem` — kopier fra hjemme-PC-en, ELLER lag et nytt par
  i appen (⚙ Innstillinger → Revolut X → 🔑 Generer nøkkelpar) og registrer
  den nye offentlige nøkkelen hos Revolut X.
- Oppdater **IP-hvitelisten** hos Revolut til ferie-nettets IP
  (sjekk <https://ifconfig.me> i nettleseren) — hotell/mobilnett har annen
  IP enn hjemme.
- I `config.toml`: `mode = "live"`, `broker = "revolutx"`, `live_ok = true`
  og riktig `[revolutx] private_key_path`.

---

## Ærlig ferieråd 🏖️

Kjør **papirmodus + Ollama** (steg A–D) på ferie. Da kan du utforske alt
gratis, uten stress med nøkler og IP-hvitelister på fremmed nett. Live-handel
med ekte penger fra et hotellnett er unødvendig risiko — ta det når du er
hjemme igjen.

**Vil du ha porteføljen/historikken din med?** Kopier `b-rs.db` fra hjemme-
PC-ens `target\release`-mappe til samme sted på den nye PC-en. Ellers starter
appen blank (helt greit for å teste).

---

## Oppdatere til nyeste versjon senere

Står du allerede i `b-rs`-mappen på riktig gren:
```
git pull
cargo build --release
```
Start så `.\target\release\b-rs-gui.exe` på nytt.
