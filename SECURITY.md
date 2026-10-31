# Security Policy

Pantau can stop processes and delete cache folders on your Mac, so a bug here can cost someone work. Please report problems privately.

## Reporting a vulnerability

Use [GitHub's private vulnerability reporting](https://github.com/fizipan/pantau/security/advisories/new). Please don't open a public issue for security problems.

Include what you found, how to reproduce it, and which version or commit you tested. Don't test against processes or folders you can't afford to lose.

This is a side project maintained in spare time, so I can't promise a response time, but I will take reports seriously.

## What counts

- A process being killed, or anything being deleted, without a click from the user.
- A delete outside the registered cleanup locations.
- A kill path that bypasses `authorize`.
- Any network request. Pantau makes none.
- Script injection (XSS) through a process name, command line or path shown in the panel.

## Supported versions

Only the latest commit on `main` is supported.
