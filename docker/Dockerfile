FROM alpine:3.24

# Install system packages, partially copied from
# https://github.com/ocrmypdf/OCRmyPDF/blob/main/.docker/Dockerfile.alpine
RUN apk --no-cache update \
  && apk --no-cache upgrade \
  && apk --no-cache add \
    font-noto \
    ghostscript \
    git \
    imagemagick \
    jbig2dec \
    jbig2enc \
    pngquant \
    python3 \
    tesseract-ocr \
    tesseract-ocr-data-chi_sim \
    tesseract-ocr-data-deu \
    tesseract-ocr-data-eng \
    tesseract-ocr-data-fra \
    tesseract-ocr-data-osd \
    tesseract-ocr-data-por \
    tesseract-ocr-data-spa \
    tiff-tools \
    ttf-droid \
    unpaper \
    uv

# Put the venv somewhere the non-root user can read
ENV UV_PROJECT_ENVIRONMENT=/opt/ocrmypdf-venv \
    UV_PYTHON_DOWNLOADS=never \
    UV_LINK_MODE=copy

# Install OCRmyPDF from sources to get an up-to-date version (this will use
# the lockfile from the source repo, in contrast to the release on pypi)
ARG OCRMYPDF_VERSION=v17.12.1
RUN git clone -b $OCRMYPDF_VERSION --depth 1 https://github.com/ocrmypdf/OCRmyPDF.git /tmp/ocrmypdf \
  && cd /tmp/ocrmypdf \
  && uv sync --frozen --no-dev --no-editable \
  && ln -s /opt/ocrmypdf-venv/bin/ocrmypdf /usr/local/bin/ocrmypdf \
  && rm -rf /tmp/ocrmypdf
