# Running it on a schedule

With the authors you follow and your trees in the store, one timer keeps
everything current, and `news` then shows what changed since you last
looked. A quiet incremental sync is one request per subscription, so
every two hours is plenty. (A running `patchtrack bridge` syncs on its
own; the timer still checks your trees.)

## Linux: a systemd user timer

```ini
# ~/.config/systemd/user/patchtrack.service
[Unit]
Description=patchtrack: sync mail and check trees

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'patchtrack sync && patchtrack check --fetch'
```

```ini
# ~/.config/systemd/user/patchtrack.timer
[Unit]
Description=Run patchtrack every two hours

[Timer]
OnCalendar=0/2:00
RandomizedDelaySec=10m
Persistent=true

[Install]
WantedBy=timers.target
```

Then:

```sh
systemctl --user enable --now patchtrack.timer
```

If `patchtrack` is not on the user manager's `PATH` (see
`systemctl --user show-environment`), write its full path, e.g.
`%h/.cargo/bin/patchtrack` after `cargo install`.

## Windows: the Task Scheduler

```bat
schtasks /Create /TN patchtrack /SC HOURLY /MO 2 ^
    /TR "cmd /c patchtrack sync && patchtrack check --fetch"
```

Give the full path to `patchtrack.exe` if it is not on the `PATH` the
scheduler uses. `schtasks /Run /TN patchtrack` runs it once to try.
