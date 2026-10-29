# The web app, with your computer or your server

The web app shows the store of a machine of yours. It keeps nothing of its
own (only the address of that machine and a token), and it cannot read
lore itself (lore sends no CORS headers, and we do not ask it to). So
`patchtrack bridge`, on your computer, serves it: the mail, what you
follow, your settings and drafts, all in the same store the command line
uses there. The bridge syncs that store by itself, and, if you let it,
sends your mail and runs b4.

## On your computer: `patchtrack bridge`

```sh
patchtrack bridge --allow-send --repo ~/linux
```

It listens on 127.0.0.1 only and prints a link that opens the web app and
pairs it. Then:

- the bridge **syncs on its own**: when it starts, every `sync_every`
  minutes (30 by default; 0 syncs only when asked), as soon as you follow
  something, and when you press "Sync now" on the Following page. The app
  notices whatever changes the store (a sync, a command you run in a
  terminal) within seconds, and shows it without a reload;
- mail you drop on the Add mail page is imported into that store, and the
  mailboxes you added with `patchtrack source add` are read on every sync;
- with `--allow-send`, a draft (a reply, a review tag) has a Send button:
  the app shows whom it goes to and asks once more, and the bridge sends
  it with `git send-email`, so your `sendemail.*` configuration and
  credentials apply. Only mail from your identities goes out;
- with `--repo`, the Branches page lists your b4 prep branches beside the
  series they posted, and runs `b4 send`, `b4 send --resend` and
  `b4 trailers -u` on them: each first as a dry run, whose output you read
  before the button that does it for real. b4 runs in a worktree of its
  own, so your checkout is never switched.

To keep the bridge running, start it from a systemd user service or the
Task Scheduler, as [Running it on a schedule](scheduling.md) does for
`sync`.

## From anywhere: your desktop's bridge, from your phone

Leave the bridge running on your desktop and use the published web app
from your phone, wherever you are. The phone's browser needs to reach the
bridge over HTTPS: a page served over https may use plain http only on
its own computer. So put a tunnel or a proxy in front of the bridge that
gives it an https address, and tell the bridge that address. The bridge
itself still listens on 127.0.0.1 only.

With Tailscale (the phone and the desktop on your tailnet):

```sh
tailscale serve --bg 47130          # https://desktop.YOUR-TAILNET.ts.net -> 127.0.0.1:47130
patchtrack bridge --public-url https://desktop.YOUR-TAILNET.ts.net --allow-send --repo ~/linux
```

Cloudflare Tunnel, or Caddy on a machine with a domain, work the same
way: anything that speaks HTTPS and forwards to 127.0.0.1:47130 keeping
the Host header.

The bridge prints a second link, "From anywhere": open it on the phone.
The app asks before it connects to a bridge elsewhere, naming it, so a
link from someone else cannot point it at their server. Keep the link to
yourself: whoever has its token can read and change the store through the
bridge (with `--allow-send`, send mail from your identities);
`--new-token` makes a new one. Where the address is on a private network
(a tailnet's), the browser may also ask once whether the site may reach
it.

## Your own copy of the app: `patchtrack serve`

`serve` is the bridge listening for other machines that also serves the
web app itself, so the app and its API share one address: no CORS, no
question about the local network, and no reliance on the published site.

```sh
# The web app: the release's web archive, unpacked.
patchtrack serve --listen 127.0.0.1:8730 --public-url https://patchtrack.example.org \
    --app-dir /srv/patchtrack/web --allow-send
```

It speaks plain HTTP: put it behind a proxy that speaks HTTPS. With Caddy:

```
patchtrack.example.org {
    reverse_proxy 127.0.0.1:8730
}
```

Open the link it prints on your phone. Every request but the app's own
files needs the token that link carries, and requests naming another host
than `--public-url`'s are refused. It syncs its store on its own, as the
bridge does.
