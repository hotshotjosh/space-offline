# Tuning: every multiplier, scale and rate

This page is for a server operator: you know EVE and you can start the server
with flags ([getting-started.md](../guides/getting-started.md)). It lists every
number that sets how fast, how rich or how hard the world is, where you can
change it, and where you cannot yet. It was checked against the code at commit
`a9f9b8f3`. A line number is a place to look, not a promise: it moves.

## How to read this

- **Takes effect** says when a change counts:
  - *live*: at once, from a GM command, and kept in the world file;
  - *next start*: restart the server (or eve-bots);
  - *new worlds only*: the value is fixed when the world file is first made;
  - *re-extract*: the value goes into the content bundle (`out/content`) when
    `tools/content.py` runs. `tools/up.sh` runs it by itself when the file has
    changed; with a hand-started server, run the tool and restart.
- **Basis** marks how sure the code is of a number: **[V]** read from the
  client, **[I]** inferred, **[A]** an assumption somebody can overturn. An
  **[A]** is a starting guess, not a rule.
- Source paths are short. `srv/` is `server/crates/eve-server/src/`, `econ/` is
  `server/crates/eve-mod-econ/src/`, `world/` is
  `server/crates/eve-mod-world/src/`, `bot/` is `server/crates/eve-bot/src/`.
- `/world` and the `--world-*` flags exist only in `eve-offline-server`, the
  server the tools start by default. The bare `eve-server` has no modules
  (`server/crates/eve-offline-server/src/main.rs`). The same is true of
  `/geode` and `/motherlode`, and of everything in `econ/tunables.rs` (the
  drop budgets, geodes, motherlodes and ambushes) and the geode-loot table.
- Every launch flag and `EVE_*` variable is in
  [launch-and-env.md](launch-and-env.md). This page gives what each number does
  and the range; that page gives the exact flag text.

## Set at launch

### The seven scales

All seven are read once, when the server starts. A bad value stops the start
with a message; it is never turned into 1. A flag wins over its `EVE_*`
variable.

| Flag (variable) | Scales | Default | Range | What it leaves alone | Source |
|---|---|---|---|---|---|
| `--time-scale N` (`EVE_TIME_SCALE`) | the slow game timers: industry jobs, PI cycles, the daily rollover (and so asteroid regrowth and site expiry), anomaly respawn, market order life, contract and courier deadlines, insurance life, skill training | 1 | whole number 1..=60 | physics, warp, module cycles, mining cycles, NPC brains, belt rat respawn, wormhole lifetimes, market history, timers under a minute. The full list is in [clock.md](../lanes/clock.md) | `srv/game_clock.rs:51,62` |
| `--training-scale N` (`EVE_TRAINING_SCALE`) | skill training only, on top of `--time-scale` | 1 | whole number 1..=10000 | everything else | `srv/game_clock.rs:54,58` |
| `--bounty-scale X` (`EVE_BOUNTY_SCALE`) | NPC bounties, live and settled offline | 1 | 0..=1000 (0 switches the faucet off) | the system's bounty pool drains by the unscaled amount; the client shows the unscaled bounty; the ESS cut comes out of the scaled amount | `srv/econ_scale.rs:46,50` |
| `--mission-scale X` (`EVE_MISSION_SCALE`) | agent mission ISK | 1 | 0..=1000 | the offer window shows the unscaled ISK; LP and standing gains | `srv/econ_scale.rs:51` |
| `--npc-buy-scale X` (`EVE_NPC_BUY_SCALE`) | what NPC buy orders pay (Tier A bids) | 1 | 0..=1000 | NPC stock; Tier B loot bids (where LP-store items sell). A raised bid stops at the larger of its 1.0 price and the type's retail ask | `srv/econ_scale.rs:52` |
| `--insurance-scale X` (`EVE_INSURANCE_SCALE`) | insurance payouts | 1 | 0..=1000 | premiums; the quote window shows the unscaled price. A payout stops at what a full policy pays | `srv/econ_scale.rs:53` |
| `--isk-scale X` (`EVE_ISK_SCALE`) | master: multiplies the four ISK scales above | 1 | 0..=1000 | ISK moving between players is never scaled; a fee, premium or tax is never scaled | `srv/econ_scale.rs:54` |

The range of the four ISK scales and the master, 1000, is an **[A]**
(`srv/econ_scale.rs:44-46`). Skills train at `time-scale x training-scale x
world training` times the base pace (`srv/rules.rs:221`).

**The alliance fee.** The fee to found an alliance is a fixed 1,000,000,000 ISK
(`ALLIANCE_CREATION_COST`, `srv/alliance.rs:54`), taken from the founding
corporation's Master Wallet. No flag, scale, `/world` setting or table changes
it. The ISK scales change only how fast a world earns toward it; the header of
`srv/econ_scale.rs` names the 1B fee as the reason they exist. To make the fee
itself settable, see [Known gaps](#known-gaps).

### World settings (`--world-*` and `/world`)

A world file is made once. The `--world-*` flags give the values when it is
made. After that the stored value wins, and a flag that differs only prints a
line saying so. Without `--world FILE` every start is a new world, so the flags
count every time. A value outside its range stops the start (`srv/rules.rs`
`validate`).

| Setting | Flag | `/world` verb | Default | Range | Takes effect | Source |
|---|---|---|---|---|---|---|
| Skill training multiplier, times the clock's pace | `--world-training` | `/world training <x>` | 1 | 1..=10000 (fractions allowed) | live; every queue is re-timed and online pilots are told | `world/settings.rs:42-51`, `srv/rules.rs:105` |
| ISK a new character starts with | `--world-start-isk` | `/world startisk` is read-only | 1,000,000 | 0 or more | new worlds only | `world/settings.rs:52-61`, `srv/start.rs:63` |
| Unallocated SP a new character starts with | `--world-start-sp` | `/world startsp` is read-only | 0 | whole number 0..=1,000,000,000 | new worlds only | `world/settings.rs:62-71`, `srv/rules.rs:85` |
| Seconds an Upwell structure takes to anchor | `--world-anchor-time` | `/world anchortime <s>` | 3600 | whole number 60..=604800 | live; structures anchored from then on | `world/settings.rs:72-81`, `srv/rules.rs:77-81` |
| Seconds between asking to delete a character and the delete | `--world-delete-time` | `/world deletetime <s>` | 3600 | whole number 0..=2592000 | live; from the next request | `world/settings.rs:82-91`, `srv/rules.rs:71-74` |

- `/world` alone lists every setting. `/world startisk` and `/world startsp`
  are refused with a message; `training`, `anchortime` and `deletetime` apply
  and are stored.
- The start ISK of 1,000,000 is the owner's choice for now
  (`srv/start.rs:63`). The one-hour delete wait is an **[A]** (`srv/rules.rs:69-71`);
  the anchor time of one hour is the owner's choice (`srv/rules.rs:76-77`). The
  anchor, delete and start SP ranges are sanity bounds, **[A]**
  (`srv/rules.rs:72-85`); the training range is the clock's
  `MAX_TRAINING_SCALE` (`srv/game_clock.rs:54`).
- Under `--dev` a new character gets 1,000,000,000 ISK whatever the setting says
  (`srv/start.rs:437,649`).
- `--concord` / `--no-concord` and `--no-undock-invuln` switch whole rules on or
  off at launch. They are not numbers; see launch-and-env.md.

## Live GM commands

These run in local chat from a GM account (see [gm-commands.md](../guides/gm-commands.md)).
Only some change a rate that stays.

| Command | What it changes | Range and default | Kept? | Source |
|---|---|---|---|---|
| `/world <name> <value>` | the live world settings above | see the table above | yes, in the world file | `world/settings.rs:197-247` |
| `/econ burn on\|off` | whether the daily rollover burns bot wallet balance above 50,000,000 ISK | off | yes | `srv/econ_ledger.rs:63-69,599-611` |
| `/fwcapture <s>` / `/fwcapture reset` | the faction warfare capture cycle (also 1 point, 10 km range, looping) | 1..=86400 s; default 60 s **[A]** | no, back to default at restart | `srv/fw_capture.rs:95-103,365-397` |
| `/dock-capitals on\|off` | whether Capital hulls dock at null-sec NPC stations | on **[A]** | no, back to default at restart | `srv/dock_policy.rs:64-66,208-226` |
| `/motherlode begin\|end\|status` | opens or closes a motherlode event in your system | no number | the event, not a setting | `econ/motherlode.rs:747-797` |
| `/whmass <pct>` | cuts the mass left on every hole in your system to at most that percent of its maximum (never refills) | 1..=100 | one-off | `srv/wormhole_jump.rs:392-436` |
| `/sovfuel burn <days>` | runs the hub fuel burn now for that many game days | whole number above 0 | one-off | `srv/service/sov_hub.rs:152-170` |

`/geode give [tier]`, `/jbridge`, `/rollover`, `/jumpfatigue 0` and the other
commands in gm-commands.md act once; they do not set a rate.

There is no live command for the seven scales. They need a restart.

## Authored tables

The tables we invent live in `content/`; each column is described in the
section of [schema.md](../content/schema.md) named below. How a table is read:

- **runtime**: the server reads it from `--authored DIR` (default `content`) at
  start. Restart to change it. A missing file uses the copy compiled into the
  binary. A wrong header, or a row it cannot parse, is logged with its line
  number; a wrong header is an error. The startup `[authored]` lines say which
  copy was used.
- **re-extract**: read by `tools/content.py` into the bundle, as above.
- **compiled-in only**: the binary holds the only copy. A change needs a
  rebuild.

| File | What it tunes | Schema section | How read |
|---|---|---|---|
| `content/population.tsv` | how many anomaly and signature sites each band holds, their respawn and life (`count`, `respawn_s`, `ttl_s`) | `content/population.tsv` | runtime |
| `content/agents/rewards.tsv` | mission ISK, bonus, LP and standing gains per level (`k_isk` 1.0 to 4.0 for security, 0.25 to 1.0 for distribution; `k_lp` 0.5) | `content/agents/rewards.tsv` | runtime |
| `content/econ/cost-index.tsv` | industry job fee index per system and activity (shipped default 0.04) | `content/econ/cost-index.tsv` | runtime, and only with `--content` |
| `content/econ/hubs.tsv` | which stations are Tier B loot-bid hubs | `content/econ/hubs.tsv` | runtime |
| `content/econ/starter-fit.tsv` | the rookie ship's fit | `content/econ/starter-fit.tsv` | runtime |
| `content/econ/geode-loot.tsv` | what a geode holds, by role weight, once geode-picks gives crate types (minerals 70, rare 15, plex 6, small injector 3, large injector 0.5, skin 1, crystal 4.5) | `content/econ/geode-loot.tsv` | runtime (`--authored`), the module `eve-mod-econ` |
| `content/facwar/warzones.tsv` | the warzones | `content/facwar/warzones.tsv` | runtime |
| `content/wormholes/statics.tsv` | the static hole of each wormhole system | `content/wormholes/statics.tsv` | runtime |
| `content/pochven/filaments.tsv` | where each Triglavian filament lands | `content/pochven/filaments.tsv` | runtime |
| `content/pve/filament-drops.tsv` | filament drop chance, parts per million; a row `off` in `source` turns drops off | `content/pve/filament-drops.tsv` | runtime |
| `content/pve/abyss-loot.tsv` | abyssal cache chance, quantity; a row `off` in `tier` turns it off | `content/pve/abyss-loot.tsv` | runtime |
| `content/sov/star_power.tsv` | power a star adds to a hub (shipped 300 for every star) | `content/sov/star_power.tsv` | runtime; a client restart is also needed |
| `content/sov/planet_resources.tsv` | power, workforce and reagent per planet type | `content/sov/planet_resources.tsv` | runtime; clients re-ask by themselves |
| `content/store/categories.tsv` | the store's category tree | `content/store/*.tsv` | runtime and re-extract |
| `content/store/offers.tsv` | what the store sells and for how much PLEX | `content/store/*.tsv` | re-extract |
| `content/econ/price-overrides.tsv` | fixed reference prices | `content/econ/price-overrides.tsv` | re-extract |
| `content/econ/recipe-markup.tsv` | the markup on built items' reference price (shipped 1.15 **[A]**) | `content/econ/recipe-markup.tsv` | re-extract |
| `content/econ/pi-markup.tsv` | the markup on PI items (shipped 1.15 **[A]**) | `content/econ/pi-markup.tsv` | re-extract |
| `content/econ/meta-mult.tsv` | variant price against its parent, by meta group | `content/econ/meta-mult.tsv` | re-extract |
| `content/agents/lp-overrides.tsv` | LP and ISK cost of single LP-store offers | `content/agents/lp-overrides.tsv` | re-extract |
| `content/agents/emblem-prices.tsv` | LP price of emblem offers | `content/agents/emblem-prices.tsv` | re-extract |
| `content/econ/geode-picks.tsv` | which crates are geodes, and their tier. It ships empty, so there are no geodes until you add picks. Put yours in `out/geode-picks.local.tsv` (same format, kept out of git), then re-extract | `mining/geodes.tsv` | re-extract |
| `content/npc/region-pirates.tsv` | which pirate faction holds a region | `content/npc/region-pirates.tsv` | re-extract |
| `content/wormholes/combat-sites.tsv` | the class of each wormhole combat site by name | none yet | re-extract |
| `content/dailygoals/goals.tsv` | the day's goals: target, draw weight, ISK reward (100,000 each, 500,000 for the completion bonus, all **[A]**) | `content/dailygoals/goals.tsv` | re-extract. No server code reads the bundle file at this commit (#202) |
| `content/bots/difficulty.tsv` | bot behaviour per level | `content/bots/difficulty.tsv` | eve-bots reads it at start (see Bots) |
| `content/bots/hostile.tsv` | when hostile bots engage | `content/bots/hostile.tsv` | eve-bots reads it at start |
| `content/npc/abyss-waves.tsv` | abyssal room waves | none yet | compiled-in only (`srv/abyss.rs:1178`) |
| `content/pve/abyss-weather.tsv` | abyssal weather stages and clouds | none yet | compiled-in only (`srv/abyss/weather.rs:79`) |
| `content/bots/names.tsv`, `phrases.tsv`, `population.tsv`, `dev/*.tsv` | bot names, chat, session start hours (`population.tsv`), dev rosters and presets | none yet | compiled-in only |

## Bots

Bots are the `eve-bots` program, started beside the server
([launch-and-env.md](launch-and-env.md#eve-bots)). Bots only play; none of these
changes a server rule.

| Setting | How | Default | Range | Takes effect | Source |
|---|---|---|---|---|---|
| Difficulty | `eve-bots --difficulty` (`peaceful`, `easy`, `normal`, `hard`, `brutal`, or 0..=4) | `normal` (2) | 0..=4 | next start of eve-bots | `bot/difficulty.rs:40-44,358-371` |
| The constants of each level (46 per level, such as `OBS_NOISE`, `COMMIT_STEP`, `GRUDGE_RATE`, `TRADE_MARGIN`) | `content/bots/difficulty.tsv`, read from `./content` relative to where eve-bots starts (no flag picks another folder) | the shipped table, all **[A]** | per constant | next start of eve-bots; a missing row keeps the built-in value | `bot/difficulty.rs:34-37`, `bot/bin/eve-bots.rs:739` |
| Which targets hostile bots engage | `content/bots/hostile.tsv`, and the `--hostile` flag | policy rows | engage 0 or 1 | next start of eve-bots | `bot/hostile.rs:78-85` |
| Number of bots | `--prefix P --count N`, `--name`, and the GM `/bots <count> ...` | none | | `--count` next start; `/bots` live | `bot/bin/eve-bots.rs:379-391`, `srv/slash.rs:44` |
| Reaction speed | `--pace human\|fast` | `human` | two words | next start | `bot/bin/eve-bots.rs:405` |
| Session pattern, day length | `--sessions daily\|always`, `--day-secs N` (with `--society`) | `daily` | N above 0 | next start | `bot/bin/eve-bots.rs:505-519` |
| Which hours bots log in | `content/bots/population.tsv` | a weight per hour | above 0 | rebuild: compiled-in only | `bot/driver.rs:525` |

## Compiled in

These numbers are constants in the code. There is no flag, variable, table or
command for any of them. A change needs a code change and a rebuild. A later
issue can make one settable; the file and line are here for that.

### Drop budgets (PLEX, injectors, filaments, HyperCores, booster copies, geodes)

Every number in `econ/tunables.rs` is **[A]**. A "bot multiplier" is the share of
a human's drop chance that a bot gets. A "daily budget" is what the whole world
may mint in a day before every chance is multiplied by 0.25
(`OVER_BUDGET_MULT`, `:20`).

| What | Values | Source |
|---|---|---|
| PLEX drops | bot multiplier 0.5; budget 3,000 PLEX a day; rows: elite NPC (bounty 5M or more) 40% for 25 to 100, heavy NPC (1M or more) 3% for 1 to 4 | `econ/tunables.rs:16,24,44` |
| PLEX seed | one stack of 500 at 3,000,000 ISK each, listed once, for 3,650 days | `econ/tunables.rs:69-74` |
| Injectors | bot multiplier 0.5; budget 5,000,000 SP a day; elite 15% large, 30% small; heavy 1% large, 5% small | `econ/tunables.rs:79,85,106` |
| Needlejack filaments (`eve-mod-econ`; not the table `content/pve/filament-drops.tsv`) | bot multiplier 0.5; 24 a day; elite 7%, heavy 0.7% | `econ/tunables.rs:129,136,153` |
| HyperCores | bot multiplier 0.5; 200 a day; stack of 1 to 5; elite 10%, heavy 1% | `econ/tunables.rs:176,182,186,203` |
| Booster blueprint copies | bot multiplier 0.5; 6 a day; elite 3.5% for 5 to 20, heavy 0.35% for 2 to 10 | `econ/tunables.rs:323,329,348` |
| Geodes (none exist until `content/econ/geode-picks.tsv` has picks; see the Authored tables) | 0.2% per cut, times 10 on a crit; bot multiplier 0.5; tier weights 80/17/3 (50/35/15 on a crit); 1, 2 or 3 prizes by tier; quantity 1, 1.5, 2.5 by tier; mineral prize is 5 to 20 cycles of 80 units at 0.5 refine; rare ore from the dearest 15%; PLEX prize 5 to 25 | `econ/tunables.rs:227-267` |
| Motherlode | an event every 2 h per active region, for 45 min, first after 15 min; +200% ore (rich site +100%); geode chance x5 (rich x3); echoes 3 jumps away after 5 min | `econ/tunables.rs:379-411` |
| Ambushes | first after 10 min, then every 15, 10 or 7 min by band; leader wreck: PLEX 25% for 5 to 20, small injector 5%, large 1%, geode 30% | `econ/tunables.rs:423-433` |
| Other NPC loot | ammo 50% (20 to 100 a stack), faction module from a commander 25%, deadspace module 10%, tag 50%, Clone Soldier tag 100% | `srv/npc/loot.rs:42-57` |
| Ship death | each stack and each fitted module survives into the wreck whole or not at all: 50% everywhere, 60% in Pochven | [V] constant, [A] that it is the drop roll | `srv/combat.rs:485-499` |
| Officer wreck | 1 to 2 officer modules | [A] | `srv/npc/loot.rs:47` |
| Spawns | commander lead 2% low-sec and 5% null, officer lead 0.2%, hauler lead 10%; site budget by difficulty 1 to 5: 40k, 120k, 400k, 1.2M, 3.5M | `srv/npc_gen.rs:81-100` |
| Combat sites | officer module 5%; a cleared site escalates 25%, an escalation's next rung 15%; k-space system holds a wormhole 12% | `srv/combat_site.rs:621,746,752`, `srv/wormhole.rs:141` |

### Asset safety

Held in `Rules` but with no flag, `/world` setting or file to write them
(only tests do). All **[A]**.

| What | Value | Source |
|---|---|---|
| Wait before the owner may deliver a wrap | 5 days (range 1 s to 1 year) | `srv/rules.rs:60,67` |
| Wait before the wrap moves itself to an NPC station | 30 days (not shorter than the manual wait) | `srv/rules.rs:62` |
| Delivery fee | 15% of the wrap's estimated value (range 0..=100) | `srv/rules.rs:64` |

### Market and industry fees

| What | Value | Basis | Source |
|---|---|---|---|
| Sales tax | 8%; falls 11% of itself per Accounting level | [V] | `srv/market_fees.rs:33,36` |
| Broker fee | 3%; 0.3 points per Broker Relations level; 0.03% per faction standing point, 0.02% per corporation standing point; minimum 100 ISK | [V] | `srv/market_fees.rs:39-52` |
| Order modification discount | 50% plus 6% per Margin Trading level | [V] | `srv/market_fees.rs:55,57` |
| Tier B loot bids | 0.40 / 0.60 / 0.70 of reference (min, start, max). The ask band 1.15 / 1.25 / 1.60 is defined but prices no order (`srv/econ_exchange.rs:205-208`) | [A] | `srv/econ_exchange.rs:209-210` |
| Every NPC order | lasts 90 days; refills hourly, full in 24 h | [A] | `srv/econ_exchange.rs:222-227` |
| Tier A NPC prices (the bids `--npc-buy-scale` scales) | ask 1.00 and bid 1.00 of base price; command centres 0.90; trade goods 0.94; ask floor 1.10 of the refine value; an orphan booster formula asks 20,000,000 ISK, an invention book 1,000,000 ISK | [V] / [I] / [A] | `srv/econ_exchange.rs:150-200` |
| Refine yield cap (0.55) | used only by a compile-time check and a test; changes nothing in play | [I] | `srv/econ_exchange.rs:231` |
| NPC station refine | efficiency 0.5, take 5% when the station has no row of its own | [V] modal | `srv/reprocess.rs:257,265` |
| Station repair cost | a full repair from zero health costs 10% of the item's reference price | [V] constant, [A] reading | `srv/repair.rs:17-18,36` |
| Industry: SCC surcharge | 4% of base cost (half for ME and TE research) | [V] | `srv/industry.rs:957-960` |
| Industry: security tax | 5% is defined but never charged: it applies only in a safe-sec system, and none is safe-sec here | [V] | `srv/industry.rs:1039,1644,2252` |
| Industry: NPC station tax, default SCC modifier | 0 and 1.0 | [I] / default | `srv/industry.rs:1054,1059` |
| Customs office tax | read from the bundle's type data (`npcCustomsOfficeTaxRate`); changing it needs a re-extract | [V] | `srv/planet.rs:354-356` |
| Planet import tax multiplier | 1.0 | [V] | `srv/planet.rs:4149` |
| Smallest payout a corp tax is taken from | 100,000 ISK | [A] | `srv/corp_wallet.rs:531` |
| Alliance creation fee | 1,000,000,000 ISK | [V] constant, [A] that the server charges exactly it | `srv/alliance.rs:54` |

### The bot economy's ISK limits

| What | Value | Basis | Source |
|---|---|---|---|
| Bot wallet cap (burn target, only with `/econ burn on`) | 50,000,000 ISK | [A] | `srv/econ_ledger.rs:63` |
| Most that bots pay one human a day for that human's asks | 4,000,000 ISK per human | [A] | `srv/econ_ledger.rs:958` |
| Bounty pool: base, recovery per hour, floor and ceiling of the system multiplier, global multiplier | 200,000,000 ISK; 0.04; 0.25 and 1.25; 1.0 (no controller exists) | [A] | `srv/econ_dbs.rs:42-61` |

### LP and insurance

| What | Value | Basis | Source |
|---|---|---|---|
| ISK one LP stands for in mission rewards | 1,000 | [A] | `srv/mission_rewards.rs:75` |
| Faction warfare kill LP | ISK destroyed divided by 3077 | constant [V], direction [A] | `srv/fw_lp.rs:78` |
| Insurance: base fraction, increment, six package ratios | 0.4, 0.5, 0.5 to 1.0 | [V] | `srv/service/insurance.rs:54-61` |
| Insurance: full policy as a share of hull price; cap on self-build cost; uninsured payout; index floor | 0.8; 0.9; 0.4; 0.5 | [A] | `srv/service/insurance.rs:75,87,96,102` |
| Insurance contract life | 12 weeks | [A] | `srv/service/insurance.rs:106` |

### Sov and entosis

| What | Value | Basis | Source |
|---|---|---|---|
| Hub claim grace | a hub whose claim has lapsed is removed after 2 offline game days; a launched hub that stays neutral is removed after 7 | [A] | `srv/sov_hub.rs:132,140` |
| Hub fuel burn | 240 blocks per claimed hub per game day | [A] | `srv/sov_hub.rs:127` |
| ADM bonus parts: base, capital, occupancy range | 1.0, 2.0, 1.0 to 6.0; index-to-bonus tables per index | [V] | `srv/sov_index.rs:94-100` |
| Strategic index levels | after 7, 21, 35, 65, 100 days claimed | [V] | `srv/sov_index.rs:86` |
| Index points: per NPC killed, per m3 mined | 60,000; 1,000 | [A] | `srv/sov_index.rs:106,109` |
| Index decay | 100,000 per game hour (a level fades in four days) | [I] | `srv/sov_index.rs:71,113` |
| Entosis: hub capture time and regen rate; command node time and regen | 595 s and 0.2; 235 s and 0.04 | [V] | `srv/entosis.rs:126-131` |

### Travel, loot and respawn

| What | Value | Basis | Source |
|---|---|---|---|
| Belt rat respawn after a clear | 5 minutes (not scaled by `--time-scale`) | [A] | `srv/belts.rs:87` |
| Hacking can entry count by band | high 1.0, low 1.5, null 2.0 | [A] | `srv/hacking_loot.rs:119-121` |
| Salvage success with no skill | 50%, kept between 5% and 95% | [A] | `srv/salvage.rs:130-133` |
| Jump fatigue | 60 s of activation and 600 s of fatigue per light-year; at most 18,000 s | [V] | `srv/jump_fatigue.rs:68-73` |
| Random-jump (filament) spool | 1.0 times the type's own seconds | [A] | `srv/random_jump.rs:151` |

### Bot economy inside eve-bots

What bots assume or pay. They change how bots play, not what the server charges.

| What | Value | Basis | Source |
|---|---|---|---|
| Sales tax and broker fee a bot plans with | 8% and 3% | [V] / [A] | `bot/pricing.rs:222,226` |
| Cost of one jump in a bot's profit sums | hauler 150,000 ISK, trader 15,000, shopping trip 10,000 | [A] | `bot/hauler.rs:50`, `bot/trader.rs:78`, `bot/travel.rs:70` |
| Bot corporation tax | between 2% and 15%, base 4%, raised by greed, lowered by loyalty | [A] | `bot/corp_ops.rs:43-48` |
| Weekend session length | 1.3 times a weekday | [A] | `bot/driver.rs:434` |
| Corporation founding fee a bot saves for (the server charges it too) | 1,599,800 ISK | [V] | `bot/corpmind.rs:62`, `srv/service/corp.rs:615` |

## Known gaps

- **The alliance fee has no setting.** It is fixed at 1,000,000,000 ISK
  (`srv/alliance.rs:54`). The scales only speed up income.
- **Asset safety timers and fee** have the fields in `Rules` and the
  validation, but no flag or `/world` verb. Adding three `Setting` rows in
  `world/settings.rs` would make them live.
- **All the drop budgets, chances and event timers** in `econ/tunables.rs` are
  constants. The tables that can be set cover only the geode prize weights
  (`content/econ/geode-loot.tsv`), abyssal key and random-jump filament drops
  (`content/pve/filament-drops.tsv`) and abyssal cache drops
  (`content/pve/abyss-loot.tsv`). The Needlejack filament drop has no setting:
  the `off` row of `filament-drops.tsv` does not touch it.
- **Taxes and fees** (sales tax, broker fee, industry surcharge, reprocessing,
  repair, insurance shape) are constants. There is no scale for them; the seven
  scales never touch a sink.
- **The dynamic bounty multiplier** (`DBS_GLOBAL`) is fixed at 1.0 and has no
  controller.
- **No live command for the seven scales.** Changing one means a restart.
- **`--time-scale` and `--training-scale` are not stored in the world file.** A
  restart at another scale re-derives some timers (see
  [clock.md](../lanes/clock.md)).
- **`/fwcapture` and `/dock-capitals` are not kept** across a restart.
- **Several tables are compiled in only**: `content/npc/abyss-waves.tsv`,
  `content/pve/abyss-weather.tsv`, and the bot name, chat and start-hour
  tables. They have no `--authored` override, and none has a schema section.
- **`content/wormholes/combat-sites.tsv`** has no schema section.
- **Daily goals** (`content/dailygoals/goals.tsv`) are extracted but no server
  code reads them yet; the ISK figures wait on that work (#202).
- **`eve-bots` reads its tables only from `./content`.** There is no flag to
  point it at another folder, unlike the server's `--authored`. Its `--content`
  is the extracted bundle, not this folder.
- **Bot difficulty touches only bots.** No setting makes NPCs, CONCORD or prices
  harder.
