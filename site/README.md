# Kura website

The official site at `https://kura.dopejs.com` follows the same architecture as Pingo:

- React renders the application shell.
- Markdown is compiled during the build, not in the browser.
- Every canonical URL is emitted as a statically rendered HTML page and hydrated on the client.
- Search uses a generated static index under `/__kura/search-index.json`.
- Language-neutral URLs keep links and search indexing stable.
- The UI language uses the shared DopeJS preference contract: local storage key `dopejs.locale` and cookie `dopejs_locale`. On `*.dopejs.com`, the cookie is scoped to `Domain=dopejs.com`.

## Commands

```sh
pnpm --dir site dev
pnpm --dir site typecheck
pnpm --dir site test
pnpm --dir site build
pnpm --dir site preview
```

The build emits 13 canonical pages plus a `/docs/` redirect into `site/dist`. GitHub Pages deploys that directory after changes reach `main`.

The previous hash routes (`/#/docs/...`) are redirected in the client to their canonical `/docs/.../` equivalents. Rollback is a normal Git revert; there is no persistent data migration.
