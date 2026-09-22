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

# Make UV installs available for non-root users
ENV UV_TOOL_DIR=/usr/local/share/uv/tools \
    UV_TOOL_BIN_DIR=/usr/local/bin

# Install OCRmyPDF from sources to get an up-to-date version
ARG OCRMYPDF_VERSION=v17.12.1
RUN git clone -b $OCRMYPDF_VERSION --depth 1 https://github.com/ocrmypdf/OCRmyPDF.git /tmp/ocrmypdf \
  && uv tool install /tmp/ocrmypdf \
  && rm -rf /tmp/ocrmypdf
