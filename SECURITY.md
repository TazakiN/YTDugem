# Security Policy

## Supported versions

Security fixes are provided for the latest release and the `main` branch. Older portable ZIP releases are not supported.

## Reporting a vulnerability

Do not open a public issue, discussion or pull request for a security problem.

Report it privately through GitHub Security Advisories:

<https://github.com/TazakiN/YTDugem/security/advisories/new>

You can also use "Report a vulnerability" on the repository's Security tab.

Please include:

- The affected version or commit, and your operating system.
- Steps to reproduce, or a proof of concept.
- The impact you expect.

## Scope notes

YTDugem stores YouTube session cookies and API keys. They are protected with Windows DPAPI on Windows and with 0700/0600 file permissions on Unix. Areas of particular interest include credential storage, the sign-in flow, arguments passed to external programs (mpv, yt-dlp, FFmpeg), verification of runtime downloads, and local inter-process communication.

Never include cookies, `Cookie` headers, tokens or API keys in a report, log excerpt, screenshot or proof of concept. Redact them. If you have exposed a real key or session, revoke or rotate it with the provider.

Issues in third-party services (YouTube, Google, lyrics providers and similar) should be reported to those providers.
