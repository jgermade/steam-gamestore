# Logging in to GOG, and where the session is kept

## The login flow

`gamestore login` prints a GOG authorization URL, you log in in any browser, and
you paste back the address you land on (it carries `code=…`); the code→token
exchange happens locally, so the client secret never leaves the machine. GOG
publishes no device-code flow and the redirect is fixed on their side, which is
why the paste step exists — see
[`RECORD/2026-09-16.gog-login-flow-decision.completed.md`](../RECORD/2026-09-16.gog-login-flow-decision.completed.md).

## Client credentials

The OAuth2 client credentials are supplied by you, either in `config.toml`:

```toml
[auth]
client_id = "…"
client_secret = "…"
```

or in the environment, which wins over the file:

```sh
export GOG_CLIENT_ID=… GOG_CLIENT_SECRET=…
```

Whether releases should ship the well-known GOG Galaxy credentials instead is
still open (question 3 in the roadmap).

## Where the session is kept

`gamestore login` stores the tokens and refreshes them by itself; GOG rotates the
refresh token, and what comes back replaces what went in. `gamestore logout`
forgets them, and `gamestore info` prints where they are.

The platform keyring is used when there is one — Secret Service on Linux, the
credential manager on Windows. When there is not, gamestore falls back to a file
under the data directory, `~/.local/share/gamestore/sessions/gog.json`, created
`0600`:

```
gog tokens:   file (~/.local/share/gamestore/sessions/gog.json, unencrypted, 0600)
              the platform keyring is not usable: No default store has been set …
```

**That file is not encrypted, and anyone who can read it can use the session.** It
is written that way on purpose. Encrypting it would need a key the same machine
can read unattended, which is obfuscation rather than secrecy; `0600` gives the
same real protection without claiming more. On a machine booted straight into
gamescope there is no D-Bus session and therefore no keyring, so this is the
ordinary path there, not a corner case — which is why `info` and `login` say so
rather than falling back quietly.

`GAMESTORE_ACCOUNT` names the session, so two Steam users sharing a machine keep
separate GOG logins (`GAMESTORE_DATA_DIR` separates their files, and the account
name separates their keyring entries, which are per-machine).
