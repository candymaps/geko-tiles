# The image of this module for the operator mode («Una imagen por módulo», docs/OPERATOR_MODE_PLAN.md
# of the host): the image of geko, which carries no module, with the package of this one, which it
# reads from GEKO_MODULES and never downloads. The build context is the folder of ci/modules.sh build.
ARG HOST=ghcr.io/candymaps/geko:latest
FROM $HOST
# Links the image to this repository in GitHub's registry, instead of that of the image of geko.
LABEL org.opencontainers.image.source=https://github.com/candymaps/geko-tiles
LABEL org.opencontainers.image.description="geko with the tiles module"
COPY tiles /usr/lib/geko/modules/tiles
ENV GEKO_MODULES=/usr/lib/geko/modules
ARG VERSION=unknown
ARG REVISION=unknown
# The image of geko it was made on, which ci/modules.sh refresh-image compares.
ARG BASE=unknown
LABEL org.opencontainers.image.version=$VERSION
LABEL org.opencontainers.image.revision=$REVISION
LABEL org.opencontainers.image.base.digest=$BASE
