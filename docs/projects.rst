Other projects
==============

The defaults read kernel-style mail: "Applied to …", ``Reviewed-by:``.
Other projects say these things differently. A **profile** teaches
patchtrack a project's phrases. It applies to the series sent to the
project's addresses.

gcc, glibc, binutils, gdb
-------------------------

The built-in ``sourceware`` profile applies to series sent to
``*@gcc.gnu.org`` or ``*@sourceware.org``. A reviewer's bare "OK.",
"LGTM." or "OK for trunk" counts as an approval. "Pushed as r17-1234" or
"Committed to trunk" counts as applied, even from the author, who commits
there. On a week of gcc-patches this took the series counted as reviewed
from 1 to 16, and applied from 0 to 7.

git
---

git reports progress in the maintainer's "What's cooking" mails, not in the patch
threads, so let the tree check follow it. git's branches map onto the
roles:

.. code-block:: toml

   [[trees]]
   name = "git-seen"
   repo = "~/git"
   url = "https://git.kernel.org/pub/scm/git/git.git"
   branch = "seen"
   role = "maintainer"

   [[trees]]
   name = "git-next"
   repo = "~/git"
   url = "https://git.kernel.org/pub/scm/git/git.git"
   branch = "next"
   role = "next"

   [[trees]]
   name = "git-master"
   repo = "~/git"
   url = "https://git.kernel.org/pub/scm/git/git.git"
   branch = "master"
   role = "mainline"

Your project
------------

Add a ``[[profiles]]`` table (see :doc:`configuration`) with the
project's addresses, and regular expressions for its approvals and its
"it is in" lines. The expressions are matched against lines that start a
paragraph. A profile with the same name as a built-in one replaces it.

Archives
--------

patchtrack reads public-inbox archives: lore.kernel.org,
inbox.sourceware.org, public-inbox.org and others (``--lore``). Mailman
archives and PostgreSQL's are not public-inbox. DPDK's archive puts its
search behind a bot check, and patchtrack does not work around those. For
any of these, your own mailbox works (:doc:`commands/import`).
