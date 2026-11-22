# Security

## Reporting a problem

Please don't open a public issue for a security problem. Use GitHub's private
[security advisory form](https://github.com/daigonstar/lodestone/security/advisories/new) instead, so it can
be fixed before it's public. Lodestone is maintained in spare time, so a reply may take a few days, but
security reports come first.

## Supported versions

Only the newest release gets fixes. Lodestone is in beta (0.1.x).

## What's protected and how

The README's [Security and privacy](README.md#security-and-privacy) section explains what Lodestone stores,
who it talks to and what it changes on your system. [AGENTS.md](AGENTS.md) holds the rules every change
must follow, and [network-hosts.txt](network-hosts.txt) lists every web address in the code, checked on
every push.

## Reviews

Each security review is written up in [docs/security-reviews](docs/security-reviews/): every finding,
how serious it was, what was changed and how the fix was checked, including what was looked at and found
fine, and what's known but not fixed.

- [2026-11-22](docs/security-reviews/2026-11-22.md): first full review (8 findings, all fixed)
