# GM commands

GM slash commands are typed in local chat by a GM character. Make a character's
account a GM with `tools/account.sh grant <NAME> gm` while the server is stopped
(in game, `/grant <username> gm` does the same for another account).

The list below is generated from the `COMMANDS` table in
`server/crates/eve-server/src/slash.rs`, which is the one source: a command that
is not in that table or a module's `commands` list answers "not implemented yet". The reply is printed in the
client's console, so each command says what it did. To refresh this table, read
the `usage` strings in that file; a new command adds one `COMMANDS` row there.

| command | usage |
|---|---|
| `/bots` | `/bots <count> [allied\|hostile] <kind> [in <system>] \| list \| despawn <group\|all> \| bring [group] \| pause\|resume <group\|bot> \| inspect <bot> \| stop-all \| help` |
| `/giveisk` | `/giveisk [character] <amount>` |
| `/pushtest` | `/pushtest [seconds]` |
| `/tp` | `/tp [km]` |
| `/whoami` | `/whoami` |
| `/giveskill` | `/giveskill <skill\|typeID> [level] \| /giveskill all [level]` |
| `/removeskill` | `/removeskill <skill\|typeID> [level]` |
| `/givestarterskills` | `/givestarterskills` |
| `/createitem` | `/createitem <type\|id> [quantity]` |
| `/dynamicitem` | `/dynamicitem <source type> <mutator type> [roll 0..1]` |
| `/setstanding` | `/setstanding <fromID> <me\|characterID> <value>` |
| `/entity` | `/entity deploy <qty> <typeID> \| /entity siege <structureID>\|off` |
| `/npcclear` | `/npcclear [radius <km>\|system] [spawned\|rats\|wrecks\|all]` |
| `/missioncomplete` | `/missioncomplete <agentID\|all>` |
| `/settle` | `/settle [park <career> \| <game hours> [name] \| sweep]` |
| `/whjump` | `/whjump <class 1-6 \| systemID \| fx \| fxmod>` |
| `/whmass` | `/whmass <percent 1-100>` |
| `/sigresolve` | `/sigresolve` |
| `/abyss` | `/abyss enter <tier 0-6> \| leave \| status \| clear \| cloud [clear] \| expire <seconds>` |
| `/escalate` | `/escalate [dungeonID]` |
| `/live` | `/live [systemID]` |
| `/rollover` | `/rollover [now]` |
| `/essdeposit` | `/essdeposit <isk>` |
| `/econ` | `/econ [regionID \| report [regionID] [hours] \| tsv [hours] \| price-tape [regionID] [days] \| audit [days] \| price <typeID> \| history [days] \| closed [days] \| burn on\|off]` |
| `/grant` | `/grant <username> <player\|gm>` |
| `/sov` | `/sov set <system\|here> <alliance\|none> \| /sov clear <system\|here> \| /sov capture <system\|here> <alliance> \| /sov info [system\|here] \| /sov go <system\|here>` |
| `/fwcapture` | `/fwcapture [<seconds> \| reset]` |
| `/sovfuel` | `/sovfuel [status [system\|here]] \| /sovfuel <blocks\|all> \| /sovfuel burn <days>` |
| `/givelp` | `/givelp <corpID> <amount>` |
| `/secstatus` | `/secstatus [characterID] <value>` |
| `/flag` | `/flag suspect\|criminal [characterID]` |
| `/dock-capitals` | `/dock-capitals [on\|off]` |
| `/jumpfatigue` | `/jumpfatigue 0 [characterID]` |
| `/devskills` | `/devskills all5 \| ids <id,id,..> \| hull <typeID>` |
| `/devcorp` | `/devcorp <ticker>` |
| `/tr` | `/tr me <stationID\|systemID\|celestialID\|shipID\|characterID> \| /tr me pos=x,y,z` |
| `/devminecrit` | `/devminecrit crit \| waste \| both \| off` |
| `/devhome` | `/devhome <system name\|id>` |
| `/upwell` | `/upwell spawn <astrahus\|raitaru\|athanor\|typeID> <name> \| /upwell give <type> \| /upwell list \| /upwell damage <id> <shield> <armor> <hull> \| /upwell state <id> <code\|name> \| /upwell timer <id> <secs> \| /upwell fit <id> <typeID> \| /upwell services <id> \| /upwell moon list`. `moon list` prints every moon extraction row (#53). `fit` puts one service module in the first free slot of the structure and onlines it, skipping the controls and the role but not the slot, hull, beacon or high-security checks (#41); `services` lists what the structure offers and what is fitted |
| `/extraction` | `/extraction advance mid\|stop\|ready\|decay` | the moon extraction window's own GM buttons (#53), on the extraction of the structure you have the controls of: `mid` puts the chunk half way, `ready` brings its arrival to now, `decay` sets the automatic fracture one second out; `stop` is not modelled and says so. Each pushes the changed extraction to the window |
| `/strfuel` | `/strfuel <structureID> hours <n>` | runs the hourly fuel burn of a structure as if `n` more hours had passed (1 to 100000; needs `/world fuelburn 1`, #42): it burns `n` hours of its online modules from the fuel bay, and takes them offline when the bay cannot pay an hour |
| `/jbridge` | `/jbridge link <gateID> <systemID> \| /jbridge unlink <gateID> \| /jbridge list \| /jbridge charge <gateID\|here> <GJ\|full>` |
| `/shutdown` | `/shutdown [seconds] [message] \| /shutdown cancel \| /shutdown status` |
| `/dailygoals` | `/dailygoals \| /dailygoals swap <slot> <key>` |
| `/skyhook` | `/skyhook state <id> <state> \| spawn <planetID\|nearest> \| activate <id> \| age <id> <cycles> \| go <id> \| info <id> \| window <id> now \| link finish <id>` |
| `/mercden` | `/mercden info <id> \| age <id> <hours> \| fill <id> \| pause <id> \| resume <id> \| go <id> \| remove <id> \| generate <den> \| complete <activity uuid> \| list <den>` |
| `/who` | `/who [filter]` |
| `/where` | `/where [characterID]` |
| `/heal` | `/heal [me\|characterID\|shipID]` |
| `/hurt` | `/hurt <armour %> <hull %> [me\|characterID\|shipID]` |
| `/suicide` | `/suicide` |
| `/cwatch` | `/cwatch [status\|clear] [characterID]` |

`/giveskill all [level]` raises every skill to `level` (default 5, 1 to 5) on any world, not only a `--dev` one, and never lowers a skill already trained higher; `/devskills all5` is the dev-world-only version (#315).

`/entity siege` orders the NPCs you deployed with `/entity deploy` in your system to besiege an Upwell structure in that system (`off` lifts the order); belt rats and the law never obey. It says how many are beyond 20 km of the structure's surface, which they do not notice. The owner gets one under-attack notification naming the NPC's corporation. Any GM session may use it, as it may use `deploy`.

Three more commands come from the modules of `eve-offline-server` (the server the
tools start by default); the bare `eve-server` does not have them. They are not
in the `COMMANDS` table but in each module's `commands` list.

| command | usage | what it does |
|---|---|---|
| `/world` | `/world [training\|startisk\|startsp\|anchortime\|freeclone\|freeupkeep\|corerule\|fuelburn\|moonyield\|deletetime <value>]` | no argument lists the world settings. `training` (a multiplier, 1 to 10000), `anchortime` (seconds, 60 to 604800), `freeclone` and `freeupkeep` (0 or 1, default 1; #41), `corerule` (0 or 1, default 1; #42: 1 keeps an anchored structure in state 102 until its quantum core is in the deed bay, 0 lets it finish onlining without one) `fuelburn` (0 or 1, default 0; #42: 1 makes every online service module burn fuel from its structure's fuel bay each UTC hour and cost fuel to online, and a bay that runs dry takes them offline; 0 keeps them free) `moonyield` (a multiplier, 0.01 to 100, default 1, retail's yield; #53: applies to moon extractions started from then on) and `deletetime` (seconds, 0 to 2592000) change at once and are kept in the world file. `startisk` and `startsp` are fixed when the world is made (`--world-start-isk`, `--world-start-sp`), and `/world` refuses to change them. A value out of range changes nothing |
| `/geode` | `/geode give [tier]` | puts a geode of that tier (default 1) in your cargo hold, if the world has geode crate types (none until `content/econ/geode-picks.tsv` or `out/geode-picks.local.tsv` has picks) |
| `/motherlode` | `/motherlode [begin\|end\|status]` | opens or closes a motherlode event on the belt last mined in your system, else its first belt; `status` lists the running ones |

The numbers behind these commands, and every other rate an operator can or cannot
set, are in [tuning.md](../reference/tuning.md).

Server flags (`--concord`, `--world-training` and the rest) are parsed in
`server/crates/eve-server/src/cli.rs` (`CORE_FLAGS`) and in each module's flag
table (`server/crates/eve-mod-world/src/settings.rs`); every flag is in
`docs/reference/launch-and-env.md`. The setup and run commands are in
`docs/guides/dev-mode.md` and `docs/guides/contributing.md`.
