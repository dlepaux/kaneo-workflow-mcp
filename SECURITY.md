# Security

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub's private vulnerability reporting](https://github.com/dlepaux/kaneo-workflow-mcp/security/advisories/new),
not in a public issue. You can expect an answer within a week. A fix is released as a
patch version, and the advisory is published once the fix is out.

Only the latest release is supported.

## What the server does with your key

- The API key is read from `KANEO_API_KEY` and sent only to `KANEO_API_URL`, as a Bearer
  header. The server never writes it to a tool result, a log line or an error message.
- Plain `http` is refused, except for loopback hosts (`localhost`, `*.localhost`,
  `127.0.0.1`, `::1`), so the key never crosses a network unencrypted.
- The server connects to that origin only. It follows no redirects, so a response cannot
  send the request on to another host. It has no telemetry and does not check for
  updates. Like most HTTP clients it honours the `HTTPS_PROXY` and `NO_PROXY`
  environment variables, so a proxy set there sees the requests.

## Verifying a release

Every release archive has a build provenance attestation from this repository's release
workflow. The README shows the `gh attestation verify` command. Run it before you
install a binary.
