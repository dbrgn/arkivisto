# Arkivisto

[![GitHub CI][github-actions-badge]][github-actions]

Your friendly CLI based workflow for scanning and archiving documents
efficiently.

**PRE-ALPHA!**

## Features

Current implementation status:

- [x] Interactive, user-friendly CLI interface
- [x] Support for multiple scanners
- [x] Scanning all from ADF
- [x] Scanning multiple pages from flatbed
- [ ] Scanning multiple pages from mixed sources
- [ ] Postprocessing
- [ ] Archiving

## History

Back in 2014, I wrote a little Python script called
[pydigitize](https://github.com/dbrgn/pydigitize) to simplify the scanning and
archival of documents. It already supported most required features, such as
scanning from ADF, straightening/cleaning of documents, running OCR on
documents, generating PDF/A files and adding keywords to these files, but the
usability of the process was not optimal. The whole scan/postprocess/archive
process was slow, so I usually had multiple command line windows open at the
same time.

After some time of using the tool regularly, I showed it to
[@ubruhin](https://github.com/ubruhin), who liked the general idea but had many
ideas on how to improve the workflow. He essentially rewrote the project and
divided the workflow into three stages: Scanning, processing, and archiving. The
project was called [docscan](https://gitlab.com/ubruhin/docscan) and proved to
be a great time saver after the initial config file setup investment.

Fast forward a few more years, docscan was still very useful, the lack of strict
types in Python made it difficult to maintain and extend the codebase. Since I
still had a few ideas on how to improve the workflow, I decided to rewrite the
project again (essentially the rewrite of a rewrite), this time using Rust. The
result is a faster, more robust and maintainable codebase that is easier to
extend and improve.

## Development Notes

### Fake Scan

During development, you can fake the scanning process with a predefined list of
documents in TIFF format. This is useful for testing and debugging purposes.

To use fake scanning, pass the `--fake-scan` flag to the arkvisto binary. Note
that the `testdata/` directory must exist in the current working directory, and
that the binary must be built in debug mode.


[github-actions]: https://github.com/dbrgn/arkivisto/actions?query=branch%3Amain
[github-actions-badge]: https://github.com/dbrgn/arkivisto/actions/workflows/ci.yml/badge.svg?branch=main
