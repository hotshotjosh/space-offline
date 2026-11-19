# Security

## Reporting a problem

Report a security problem privately, through GitHub's private vulnerability
reporting: the repository's **Security** tab, then **Report a vulnerability**. Only
you and the maintainers see it.

Never report one in an issue, a pull request, a comment or a discussion: those are
public. There is no email address for reports.

Say what you found, where (file and line, or the command), how to reproduce it, and
what it lets someone do. Leave out secrets, tokens, and anything from your own
machine that you would not publish. A maintainer answers in the report, and says
when a fix has landed.

## In scope

- the server (`server/`): anything a client or a bot can send that crashes it, reads
  or changes another account, or gets past a role check;
- the admin and GM surfaces: a way in without the right account or role;
- the client jail and its helper (`tools/install-jail-helper.sh`, the `tools/jail-*.sh`
  scripts, `docs/guides/jail.md`): a route out of the jail, or root access beyond
  what `tools/install-jail-helper.sh --diff` shows it installs;
- the tools a contributor runs: a script, a container (`tools/dev.sh`,
  `tools/analysis.sh`, `containers/`) or the gate that runs code outside its
  container, writes outside the checkout and its own temp files, or uses the network
  when it should not;
- the GitHub workflows (`.github/`): a way for a pull request to run code with a
  write token, to set a label its checks did not earn, or to reach secrets;
- anything in the repository that should not be there: a secret, a key, personal
  data, or material from the game client.

## Out of scope

- the EVE Online client itself, and CCP's services: report those to CCP, not here;
- your own hosting choices for a server you run (a port you open, a firewall you
  leave off);
- third-party projects this one cites; report those to them.

This is a hobby project with no money in it (`LICENSE-additions.md`): there is no
bounty, and nobody receives anything for a report.
