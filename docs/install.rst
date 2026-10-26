Installing
==========

patchtrack needs Python 3.11 or later and nothing else from PyPI. The
``check`` and ``branches`` commands also need ``git``.

.. code-block:: sh

   pip install patchtrack
   patchtrack --version

From a checkout, ``pip install .`` does the same.

To run it without installing, from the top of a checkout:

.. code-block:: sh

   PYTHONPATH=src python3 -m patchtrack --help

The store is created on first use at
``$XDG_DATA_HOME/patchtrack/patchtrack.sqlite3`` (``--db`` names another),
and the optional config file is read from
``$XDG_CONFIG_HOME/patchtrack/config.toml`` (see :doc:`configuration`).
