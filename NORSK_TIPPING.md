# Norsk Tipping-prosjektet

Et idé- og plandokument for en **Norsk Tipping-modul** i b-rs: et verktøy for å
følge med på spill, trekninger og eget forbruk hos Norsk Tipping — med samme
filosofi som resten av appen: **ærlige tall, harde grenser og full oversikt**.

> ⚠️ Dette er et plandokument, ikke ferdig funksjonalitet. Ingenting av dette
> er implementert ennå.

## Hva er Norsk Tipping?

[Norsk Tipping](https://www.norsk-tipping.no) er det statlige norske
spillselskapet med enerett på pengespill som Lotto, Vikinglotto, Eurojackpot,
Joker, Extra, Keno, Flax og sportsspillene Tipping og Oddsen. Overskuddet går
til norsk idrett, kultur og frivillighet, og alt spill krever spillerkort og
18-årsgrense.

## Ærlig matematikk først

b-rs viser deg ærlig om du slår børsen — den samme ærligheten gjelder her,
og den er ikke pen:

| Spill | Omtrentlig sjanse for toppgevinst | Andel av innsatsen som betales tilbake |
|---|---|---|
| Lotto (7 av 34 tall) | ca. 1 : 5,4 millioner per rekke | ca. 50 % |
| Vikinglotto | ca. 1 : 61 millioner | ca. 50 % |
| Eurojackpot | ca. 1 : 140 millioner | ca. 50 % |
| Oddsen (sportsspill) | avhenger av oddsen | typisk 80–90 % |

To ting følger av dette:

1. **Forventet avkastning er alltid negativ.** For hver hundrelapp du spiller
   for i Lotto, får spillerne samlet ca. 50 kroner tilbake. Ingen strategi,
   ingen «hete tall» og ingen statistikk endrer på det — hver trekning er
   uavhengig av den forrige.
2. **Dette er underholdning, ikke investering.** Derfor skal en eventuell
   modul aldri gi «tips om vinnertall» eller late som flaks kan systematiseres.
   Den skal gjøre det motsatte: vise deg de ekte tallene.

## Hva modulen kan gjøre

I b-rs-ånd — lesemodus, budsjett og ærlig rapportering:

- **Trekningsresultater**: hente og vise siste resultater for Lotto,
  Vikinglotto, Eurojackpot og Joker, med varsel på mobilen (via den
  eksisterende `notify`-modulen) når trekningen er klar.
- **Spillbudsjett**: et fast månedsbudsjett for spill, ført som egen post ved
  siden av spareavtalene — med fremdriftslinje og hard stopp, akkurat som
  tapsgrensen i `[risk]`.
- **Forbruksoversikt**: manuell føring (eller import) av egne innsatser og
  gevinster, slik at appen kan vise **reelt netto resultat over tid** — samme
  ærlige kurve som egenkapitalgrafen mot `^OSEAX`.
- **EV-kalkulator**: skriv inn innsats og spill, få forventet tap svart på
  hvitt før du leverer kupongen.
- **Jackpot-oversikt**: vise gjeldende førstepremiepotter, siden mange kun
  spiller når potten er stor.

## Hva modulen aldri skal gjøre

- ❌ Levere kuponger eller spille automatisk. Norsk Tipping har ikke noe
  offentlig API for spilling, og automatisert spilling ville uansett brutt
  vilkårene deres.
- ❌ Foreslå tall, «systemer» eller strategier for å vinne.
- ❌ Omgå Norsk Tippings egne grenser for innsats og tap.

## Teknisk skisse

Modulen følger samme mønster som `nordnet`-modulen: **kun lesing**, tydelig
merket, og isolert fra handelsmotoren.

| Del | Ansvar |
|---|---|
| `src/tipping.rs` | Henting av trekningsresultater og jackpotter (uoffisielle JSON-endepunkter fra norsk-tipping.no — kan slutte å virke uten varsel, som Nordnet-modulen) |
| `store` | Egne tabeller for innsatser, gevinster og trekninger i SQLite-loggen |
| `gui` | Egen fane: resultater, budsjett-fremdrift og netto-kurve |
| `notify` | Push ved trekningsresultat og ved 80 % / 100 % av spillbudsjettet |
| `config.toml` | Ny `[tipping]`-seksjon: `enabled`, `budsjett_mnd`, hvilke spill som følges |

## Ansvarlig spill

- 18-årsgrense på alt spill hos Norsk Tipping.
- Sett grenser hos Norsk Tipping selv — de gjelder uansett hva denne appen viser.
- Kjenner du at spillingen tar overhånd: **Hjelpelinjen, tlf. 800 800 40**
  ([hjelpelinjen.no](https://hjelpelinjen.no)) — gratis og anonymt.

## Status

- [ ] Avklare hvilke endepunkter for trekningsresultater som er stabile nok
- [ ] `[tipping]`-seksjon i konfig + `src/tipping.rs` med resultathenting
- [ ] Budsjett og forbruksføring i `store` + GUI-fane
- [ ] Varsler via `notify`

Som resten av b-rs: dette er et hobbyverktøy. Det gir ikke spilleråd, og
forventningsverdien i lotterispill er alltid negativ.
