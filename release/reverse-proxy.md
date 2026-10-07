# my-reverse-proxy for `task-manager.jetdev.eu`

Goes into `~/.my-reverse-proxy` on the host where the proxy runs — the same host as the stack.

```yaml
hosts:
  task-manager.jetdev.eu:443:
    endpoint:
      type: https
      ssl_certificate: task_manager_cert

    # ONE UPSTREAM. The product is a single container: the same server answers `/api`, `/mcp`, `/ws` and
    # `/raw` and serves the browser its bundle for everything else, so the proxy has nothing to tell
    # apart. There used to be four locations here and a second port for the client; the order they had
    # to be written in was the whole config, and it is gone with them.
    #
    # `/ws` is still spelled out, for its timeout and nothing else. Locations are matched by
    # case-insensitive path PREFIX and the FIRST match wins — there is no longest-prefix rule — so it
    # has to stay above `/`.
    locations:
    - path: /ws
      type: http
      proxy_pass_to: http://127.0.0.1:31500
      # The default request timeout is 15s. A board's socket is meant to stay open for hours, so it needs
      # raising — if the upgraded stream turns out to sit outside the timeout, this is harmless anyway.
      request_timeout: 86400000

    # Everything else: the API, MCP, a document's bytes, and the client with every route inside it.
    - path: /
      type: http
      proxy_pass_to: http://127.0.0.1:31500

  # Plain HTTP exists only to send people to HTTPS. The host is written out rather than taken from
  # ${HOST_PORT}, which would carry `:80` into the redirect.
  task-manager.jetdev.eu:80:
    endpoint:
      type: http
    locations:
    - type: static
      status_code: 302
      modify_http_headers:
        add:
          response:
          - name: Location
            value: https://task-manager.jetdev.eu${PATH_AND_QUERY}

ssl_certificates:
  - id: task_manager_cert
    certificate: ~/certs/task-manager.jetdev.eu.cer
    private_key: ~/certs/task-manager.jetdev.eu.key
```

## Why these upstreams

`127.0.0.1:31500` rather than the container name. The proxy is on the same host and the port is
published, so this works whether or not the proxy sits on `docker_net`. If it does, and you would rather
not publish the port at all, swap the upstream for `http://task-manager:8000` and delete the `ports:`
block from the compose — one less surface exposed on the host.

Unix sockets are not an option here: the service does create one, but my-reverse-proxy's unix-socket HTTP
connector is commented out in its source, so the upstream has to be TCP.

## Three things worth checking after the first request

**`/authorized` on a cold load.** Google sends the browser straight there, so it must return `index.html`
rather than 404. That fallback is the server's own — `UiMiddleware` answers any `GET` that is not under
`/api`, `/mcp`, `/ws`, `/raw`, `/swagger` or `/metrics` with the client's page — and the proxy only
forwards. Test it by opening `https://task-manager.jetdev.eu/authorized` directly in a new tab; a 404
means the image was built without `wwwroot/`, and the whole sign-in breaks without it. Same for
`/projects-setup`, `/users`, `/settings`.

**The WebSocket.** Open Home and look at the dot next to the project dropdown: green means the socket is
up, grey means it is not. my-reverse-proxy does handle the upgrade (it uses `hyper_tungstenite` and has an
explicit `WebSocketUpgrade` path), so this should work — the dot is there precisely so a failure is visible
instead of the board silently going stale.

**`/mcp` is exposed to the internet.** In this version it has no authorization at all — see `TODO.md`. If
that is not acceptable yet, either drop the `/mcp` location from this config and reach it over the host
port from inside the perimeter, or put a client certificate on it: the proxy supports
`client_certificate_ca` per endpoint, though that would need its own host entry since it applies to the
whole endpoint rather than one location.
