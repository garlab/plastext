# syntax=docker/dockerfile:1
# Highlighting fixture: stages, flags, variables, shell, JSON and heredocs.
ARG VERSION=3.21
FROM alpine:${VERSION} AS build
WORKDIR /app
ENV GREETING="hello"
LABEL description="Café demo"
COPY --chown=1000:1000 . /app
RUN echo "$GREETING" && \
    printf '%s\n' "ready"
RUN <<EOF
if true; then
    echo "hello from a heredoc"
fi
EOF
COPY <<EOF /app/config.json
{"enabled": true, "count": 2}
EOF

from alpine:3.21 as final
CoPy --from=build /app /app
EXPOSE 8080
CMD ["/bin/sh", "-c", "echo ready"]
