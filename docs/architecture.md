# Architecture

```text
Developer machine                         Single host
-----------------                         -----------
cella dev  -> local celld dev
cella deploy -> constrained SSH -> celld-ctl
  |                                      |  provision/enable/reload
  +-> local native celld deploy ---------+-> object-store prefix

HTTPS proxy -> Caddy :8000 -> /app/* -> celld-cell@app.service (127.0.0.1:PORT)
```

Every independently coded app gets a distinct celld fleet prefix such as `s3://BUCKET/cells/app` and a distinct local celld process. Caddy preserves the prefix: `/app/foo` reaches the app as `/app/foo`. Never expose celld internal listeners publicly.
