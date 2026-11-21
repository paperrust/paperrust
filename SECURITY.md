# Security policy

## Supported versions

PaperRust has not been released yet. Until the first release, fixes are made on the `main` branch
only.

| Version | Supported |
|---|---|
| 0.1.x (once released) | yes |
| unreleased development versions | best effort, on `main` only |

## Reporting a vulnerability

Please do **not** report security problems as public issues, pull requests or discussions.

Report them privately:

- by e-mail to **contact@paperrust.org** (until a dedicated security address exists), or
- via GitHub private vulnerability reporting ("Report a vulnerability" on the Security tab of
  `paperrust/paperrust`).

Please include, as far as you can:

- the affected version or commit, and the mode (Docker/server, desktop app, NAS template),
- the configuration and user permissions needed to trigger the problem,
- steps to reproduce (a minimal request or script is ideal),
- the realistic impact (what an attacker can read, change or break),
- whether you want to be credited, and under which name.

Do not include real personal documents or credentials. If a report was produced with the help of an
AI tool, please say so and confirm that you checked the finding against the code yourself.

## What happens next

PaperRust is a small project. We will acknowledge your report, look into it, and keep you
informed about what we find and when a fix is available. We cannot promise fixed response times. We
would ask you to give us a reasonable amount of time to fix the problem before you disclose it
publicly; we will coordinate the disclosure date with you and credit you if you wish.

## Scope

PaperRust aims to be at least as secure as paperless-ngx v3.4.2. Where PaperRust is stricter (for
example on import), this is noted as a "Known difference" in `docs/contract/*.md`. Behaviour that
paperless-ngx documents as intended (for example: users with the permission to change users can grant
themselves permissions; users who can edit a mail account can point its stored credentials to another
server) is not considered a vulnerability in PaperRust either.

Vulnerabilities in **paperless-ngx itself** that also affect PaperRust (because the behaviour was
reproduced) should additionally be reported to the paperless-ngx project according to its policy:
<https://github.com/paperless-ngx/paperless-ngx/security>. PaperRust is an independent project; the
paperless-ngx team is not responsible for it.

Secure deployment advice: [English](docs/user/en/security.md) · [Deutsch](docs/user/security.md)
