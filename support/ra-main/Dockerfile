# Debian bullseye on purpose: it ships glibc 2.31 and gcc 10, which is what
# the MiSTer rootfs has (/lib/libc-2.31.so) and what upstream's Makefile says
# it builds with. A newer Debian links against symbols the device does not
# have and the binary refuses to start.
FROM debian:bullseye
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      crossbuild-essential-armhf make ca-certificates \
 && rm -rf /var/lib/apt/lists/*
