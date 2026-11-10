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
| `/giveskill` | `/giveskill <skill\|typeID> [level]` |
| `/removeskill` | `/removeskill <skill\|typeID> [level]` |
| `/givestarterskills` | `/givestarterskills` |
| `/createitem` | `/createitem <type\|id> [quantity]` |
| `/dynamicitem` | `/dynamicitem <source type> <mutator type> [roll 0..1]` |
| `/setstanding` | `/setstanding <fromID> <me\|characterID> <value>` |
| `/entity` | `/entity deploy <qty> <typeID>` |
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
| `/upwell` | `/upwell spawn <astrahus\|raitaru\|athanor\|typeID> <name> \| /upwell give <type> \| /upwell list \| /upwell damage <id> <shield> <armor> <hull> \| /upwell state <id> <code\|name>` |
| `/jbridge` | `/jbridge link <gateID> <systemID> \| /jbridge unlink <gateID> \| /jbridge list` |
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

Three more commands come from the modules of `eve-offline-server` (the server the
tools start by default); the bare `eve-server` does not have them. They are not
in the `COMMANDS` table but in each module's `commands` list.

| command | usage | what it does |
|---|---|---|
| `/world` | `/world [training\|startisk\|startsp\|anchortime\|deletetime <value>]` | no argument lists the world settings. `training` (a multiplier, 1 to 10000), `anchortime` (seconds, 60 to 604800) and `deletetime` (seconds, 0 to 2592000) change at once and are kept in the world file. `startisk` and `startsp` are fixed when the world is made (`--world-start-isk`, `--world-start-sp`), and `/world` refuses to change them. A value out of range changes nothing |
| `/geode` | `/geode give [tier]` | puts a geode of that tier (default 1) in your cargo hold, if the world has geode crate types (none until `content/econ/geode-picks.tsv` or `out/geode-picks.local.tsv` has picks) |
| `/motherlode` | `/motherlode [begin\|end\|status]` | opens or closes a motherlode event on the belt last mined in your system, else its first belt; `status` lists the running ones |

The numbers behind these commands, and every other rate an operator can or cannot
set, are in [tuning.md](../reference/tuning.md).

Server flags (`--concord`, `--world-training` and the rest) are parsed in
`server/crates/eve-server/src/cli.rs` (`CORE_FLAGS`) and in each module's flag
table (`server/crates/eve-mod-world/src/settings.rs`); every flag is in
`docs/reference/launch-and-env.md`. The setup and run commands are in
`docs/guides/dev-mode.md` and `docs/guides/contributing.md`.
