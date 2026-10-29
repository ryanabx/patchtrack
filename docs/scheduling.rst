Running it on a schedule
========================

With authors and trees in the config, one timer keeps everything current,
and ``news`` then shows what changed since you last looked. A quiet
incremental sync is a single request, so every two hours is plenty.

A systemd user service and timer:

.. code-block:: ini

   # ~/.config/systemd/user/patchtrack.service
   [Unit]
   Description=patchtrack: sync mail, check trees, write the board

   [Service]
   Type=oneshot
   ExecStart=/bin/sh -c 'patchtrack sync && patchtrack check --fetch; patchtrack board -o %h/patchtrack-board.html'

.. code-block:: ini

   # ~/.config/systemd/user/patchtrack.timer
   [Unit]
   Description=Run patchtrack every two hours

   [Timer]
   OnCalendar=0/2:00
   RandomizedDelaySec=10m
   Persistent=true

   [Install]
   WantedBy=timers.target

Then:

.. code-block:: sh

   systemctl --user enable --now patchtrack.timer

If ``patchtrack`` is not on the user manager's ``PATH`` (see
``systemctl --user show-environment``), write its full path, e.g.
``%h/.local/bin/patchtrack`` after ``pip install --user``.

For a web frontend that reads static files, add ``export`` to the same
line, e.g. ``; patchtrack export %h/public_html/patchtrack``. A frontend
that talks to ``serve`` needs no timer for the data: each request reads
the store as the last sync left it.
