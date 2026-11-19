# Eden: how we use it

Eden (https://github.com/CJ-SPT/Eden) is an independent EVE Online server emulator in C#,
under the MIT licence ("Copyright (c) 2026 Eden contributors"). It targets EVE 24.01, Windows
build 3662168, which is not our pinned build. We read revision `a1437dc4` (2026-11-19).

## The rule

- **Facts and ideas, freely.** An Eden claim is a hint. It becomes `[V]` only once it is
  checked against our pinned client; until then it is `[I]`.
- **A fact Eden took from EvEJS follows the EvEJS rule** (AGPL: facts only, verified, never its
  code or prose). Eden cites EvEJS in many places: look for "evejs", a `.js` file name or a
  SHA-256 of an EvEJS file next to the claim.
- **Never its data.** Eden's git holds data extracted from the client and the SDE, and data
  fetched from third-party sites. None of it may enter our tree (list below).
- **Code only with a notice.** Porting Eden code to Rust needs a `NOTICE` block (the MIT text,
  the revision read, the files it went into) and an SPDX header on each file that carries it.
- **Never run anything from Eden**: no scripts, tests, builds or binaries.
- A commit that used Eden as a lead says so in a trailer: `Eden-ref: <path>:<line>@<rev>`.

## Never copy (Eden paths)

Client or SDE extracts:

- `tests/Eden.Protocol.Tests/Fixtures/*.json` (the catalog fixtures);
- `docs/*.json`, `docs/data/*.csv`, `docs/client-routes/*`;
- `docs/protocol/static-collision-*.json` and `src/Eden.Data/Catalog/**` (collision, schemas,
  exploration rosters);
- `src/**/*-3662168.json` (mining sites, content support, missions, encounters);
- `src/Eden.Protocol/Gateway/*.proto` (reconstructed from the client);
- `tools/catalog/build-*.json`, `tools/catalog/relationships-*.json`.

Third-party web data: `src/Eden.World/Market/market-prices.json`,
`src/Eden.World/Market/loyalty-offers.json`, and anything its importers fetch from market,
ESI, wiki or wormhole sites.

## How far to trust it

| area | trust |
|---|---|
| wire shapes and client rules citing a client module and line | medium-high: re-check on our build |
| space physics citing Carbon Destiny source | medium-high: read Carbon directly instead |
| server-side formulas (damage rolls, bleed-through, heat chance) | low: often community lore or Eden's own choice |
| missions, anomalies, encounters, loot | low: built from community reports or reconstructed |
| bots, economy presets, Abyssal distributions | Eden's own design, not facts |
| anything sourced from EvEJS | the EvEJS rule |

Eden's own records mostly say no live-client check was done, so treat a "native" claim as
read from the client archive, not seen working. Eden can cite the client and still be wrong
(its `warsInfoMgr.GetWarsByOwners` answer is a list where the client iterates a dict).
