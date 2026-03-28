# Arkivisto

![Logo](static/logo-v1-256.png)

Your friendly CLI based workflow for scanning and archiving documents
efficiently.

[![GitHub CI][github-actions-badge]][github-actions]

## Features

Current implementation status:

- [x] Interactive, user-friendly CLI interface
- [x] Support for multiple scanners
- [x] Scanning all from ADF
- [x] Scanning multiple pages from flatbed
- [x] Postprocessing
- [x] Archiving

## Dependencies

You need the following binaries on your system:

- `scanimage` (part of SANE)
- `magick` (part of Imagemagick)
- `tiffcp` (part of libtiff)
- `docker` (part of Docker or Podman)

## Regular Expressions

The config allows you to specify regular expressions in two locations: Titles
and dates.

For the dates, this is just a way to make date matching more reliable in case
there are multiple dates in the document, it allows you to match the correct
one.

For titles, it allows extracting information from the document and putting it
into a title pattern. For matching and replacement, the Rust
[regex](https://docs.rs/regex) crate is being used. For replacement, match
groups can be referenced using the `$` sign. For example, this config:

```yaml
pdf_title_regex: "Invoice for the year .*(20[0-9]{2})"
pdf_title_pattern: Subscription $1
```

...will match the string `Invoice for the year 2026` and result in the document
title `Subscription 2026`.

## Setup

To generate an initial config, run:

    arkivisto init-config

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

## License

Licensed under the AGPL version 3 or later. See `LICENSE.md` file.

    Copyright (C) Danilo Bargen

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU Affero General Public License as
    published by the Free Software Foundation, either version 3 of the
    License, or (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU Affero General Public License for more details.

    You should have received a copy of the GNU Affero General Public License
    along with this program.  If not, see <https://www.gnu.org/licenses/>.
