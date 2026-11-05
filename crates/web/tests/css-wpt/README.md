# Native syntax assertion ports

These two unmodified WPT resources are pinned by SOURCE.json and carry
the upstream license. The native test ports identifier-range data and
declaration-whitespace assertions. It does not execute their JavaScript,
CSSOM, style calculation or rendering, and is not reported as a page pass.
The identifier source has a missing comma between ranges. Linewise data
extraction preserves the original file and checks the intended lexer
boundaries; it does not pretend that upstream script ran successfully.
