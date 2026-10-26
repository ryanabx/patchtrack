Quick start
===========

Your patches
------------

.. code-block:: sh

   patchtrack sync -a you@example.org
   patchtrack list

The first sync searches the last three months and fetches the thread of
every series it finds; later ones fetch only what changed. ``list`` shows
one line per series family, with the evidence for its state underneath.

A config file
-------------

So that nothing needs repeating, put your address, and any git trees to
check, in ``~/.config/patchtrack/config.toml``:

.. code-block:: toml

   [sync]
   authors = ["you@example.org"]

   [[trees]]
   name = "next"
   repo = "~/linux"
   url = "https://git.kernel.org/pub/scm/linux/kernel/git/next/linux-next.git"
   branch = "master"
   role = "next"

:doc:`configuration` lists every key.

The daily round
---------------

.. code-block:: sh

   patchtrack sync && patchtrack check --fetch   # fetch mail, look in the trees
   patchtrack news                               # what changed since last time
   patchtrack nudge                              # what needs a reply, a ping or a RESEND
   patchtrack read gpio                          # read one series' conversation
   patchtrack tui                                # or browse everything

:doc:`scheduling` runs the first line on a timer.
