# Browser Capability

The real browser driver for the computer-use plane (Stage 9.4): a
supervised Node worker (`worker.mjs`) driving Chromium through Playwright,
speaking the line-JSON protocol in `PROTOCOL.md` to the daemon's
`SubprocessDriver`.

Requirements: Node 18+ and the `playwright` package with a Chromium build
(`npx playwright install chromium`). Point `KURA_PLAYWRIGHT_PATH` at the
package if it is not resolvable from this directory.

Run by hand:

```sh
echo '{"id":1,"op":"start_session","session":{"computerUseSessionId":"cus_1","runId":"run_1","status":"starting","driverKind":"browser","startedAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"},"input":{"initialUrl":"https://example.org"}}' | node worker.mjs
```
