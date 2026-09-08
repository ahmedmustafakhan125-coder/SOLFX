# web-public

Files the internet-facing container is allowed to serve, and nothing else.

`price-accounts.json` is the poster's feed-id -> price-account map. The browser needs it to
know which account holds each price, and it changes whenever the poster generates a new set
of accounts.

It lives here rather than in `app/public/` because `app/public/` is a *build-time* input:
copying a fresh map there does nothing until the app is rebuilt, and a rebuild is exactly
what nobody does after restarting the poster. The terminal then reports "No price account for
this feed on this cluster" while the poster is running perfectly — which is what happened on
2026-09-08.

The poster writes here directly (`--map-out`), `docker-compose.yml` mounts this directory
read-only, and `services/api/server.mjs` serves it ahead of the built assets. So the map the
browser reads is always the map the poster is publishing to, with no copy step to forget.

Keypairs stay in `price-accounts/`, which is deliberately NOT mounted into the container.
