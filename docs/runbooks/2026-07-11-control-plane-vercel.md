# Optimus control-plane Vercel runbook

## Production resources

- Project: `sz-747s-projects/optimus`
- Dashboard: `https://optimus-umber.vercel.app/control-plane`
- State store: the Vercel Redis integration exposed as `REDIS_URL`
- Deployment: `dpl_GSddWTFYiNvazCZ15sJ5cffQVTWT`

The dashboard and desktop use different credentials. Configure
`OPTIMUS_DASHBOARD_TOKEN` and `OPTIMUS_DESKTOP_TOKEN` as sensitive Production
variables in Vercel. Both must contain at least 24 characters. Never reuse one
for the other, commit either value, or put the dashboard token in browser
storage. Authentication exchanges it for a signed HttpOnly cookie.

## Desktop pairing

Start Optimus with these variables in its inherited environment:

```powershell
$env:OPTIMUS_RELAY_URL = "https://optimus-umber.vercel.app"
$env:OPTIMUS_DESKTOP_TOKEN = "<same desktop-only secret configured in Vercel>"
$env:OPTIMUS_DEVICE_ID = "primary"
$env:OPTIMUS_RELAY_WORKSPACES = '[{"id":"workspace-primary","label":"Optimus","repoRoot":"C:\\dev\\Optimus"}]'
$env:OPTIMUS_PROVIDER_PROFILES = '[{"id":"codex-subscription","label":"Codex subscription","command":"codex","authMode":"subscription"}]'
```

For API billing, register a separate profile with `"authMode":"api"` and an
approved `credentialEnv`, then provide that credential only to the desktop
process. Example: `"credentialEnv":"OPENAI_API_KEY"`. Subscription profiles
clear provider API-key variables before launching their child so the model CLI
uses its existing signed-in subscription.

Repository paths, executable commands, API credential values, raw tasks, Git
metadata, memory facts, and rules stay on the desktop. The relay receives only
the validated public snapshot and typed lifecycle commands. Raw web-issued task
payloads expire after two minutes and are scrubbed on terminal acknowledgement.

## Deploy

```powershell
npx vercel pull --yes --environment=production
npx vercel deploy --prod --yes --scope sz-747s-projects
npx vercel inspect https://optimus-umber.vercel.app --scope sz-747s-projects
```

The production build must finish with status `Ready`. The alias should resolve
to the new deployment before running the live gate.

## Verify

Keep paired secrets in the ignored `.env.local`, load them into the process,
then run:

```powershell
$env:PLAYWRIGHT_BASE_URL = "https://optimus-umber.vercel.app"
npm run test:web:live
npx vercel logs https://optimus-umber.vercel.app --no-follow --since 30m --level error --scope sz-747s-projects
```

The live Playwright gate checks health, unauthenticated rejection, hardened
headers, signed cookie properties, no browser token persistence, a real Redis
snapshot, SSE revision delivery, command polling, and desktop acknowledgement.
It writes `artifacts/playwright/control-plane-live-production.png`.

Before shipping desktop changes, also run:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test ui/mock/tauri-mock.test.mjs
npm run test:relay
npm run build
npm run test:web
npm run test:web:relay
npm run tauri -- build
```
