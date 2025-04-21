# Arkivisto

[![GitHub CI][github-actions-badge]][github-actions]

Your friendly CLI based workflow for scanning and archiving documents
efficiently.

**PRE-ALPHA!**

## History

TODO

## Development Notes

### Fake Scan

During development, you can fake the scanning process with a predefined list of
documents in TIFF format. This is useful for testing and debugging purposes.

To use fake scanning, pass the `--fake-scan` flag to the arkvisto binary. Note
that the `testdata/` directory must exist in the current working directory, and
that the binary must be built in debug mode.


[github-actions]: https://github.com/dbrgn/arkivisto/actions?query=branch%3Amain
[github-actions-badge]: https://github.com/dbrgn/arkivisto/actions/workflows/ci.yml/badge.svg?branch=main
