"""PaperRust parity harness (stdlib only, Python >= 3.11).

Sends identical requests to two backends (A = Django reference, B = PaperRust),
compares status, selected headers and the JSON body (deep, including key order),
normalises only volatile values, counts accepted deviations by named rule and writes a
report with history. Refuses every non-loopback target.

Modules:
    config     - targets, paths, environment
    http       - loopback-only HTTP client with cookie jar, token/basic/session auth + CSRF
    users      - user/role catalogue (tokens, passwords) from reference/
    normalize  - volatile-value normalisation (timestamps after cut-off, task ids, CSRF, ...)
    diff       - deep JSON diff incl. key order
    rules      - accepted deviations (named rules)
    leaks      - leak scan (password hashes, tokens, paths, stack traces, secrets)
    cases      - case catalogue loader (tests/parity/<crate>/cases*.json|yaml), vars, expansion
    runner     - executes cases against both targets (snapshot reset for write cases)
    report     - JSON + Markdown report, history
"""

__version__ = "1.0.0"
