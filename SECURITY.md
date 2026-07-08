# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| Latest  | Yes       |

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly:

1. **Do not** open a public issue
2. Use [GitHub Security Advisories](https://github.com/tyql688/sessionview/security/advisories/new) to report privately
3. Include a description of the vulnerability and steps to reproduce

You can expect an initial response within 72 hours.

## Scope

SessionView is a desktop app that reads local AI coding session files. Security concerns include:

- Path traversal when reading session files or images
- XSS in rendered markdown or exported Markdown content
- Command injection in terminal resume commands
- Unauthorized file access outside expected directories
