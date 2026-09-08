# Use a maintained host while retaining MiSTer's glibc 2.31 / GCC 10 target ABI.
# Bullseye supplies the GCC 10 cross compiler and ARM sysroot only; its other
# packages have lower priority than the Bookworm host libraries and tools.
FROM debian:bookworm
RUN printf 'deb http://deb.debian.org/debian bullseye main\n' > /etc/apt/sources.list.d/bullseye.list \
 && printf 'Package: *\nPin: release n=bullseye\nPin-Priority: 1\n\nPackage: *-armhf-cross\nPin: release n=bullseye\nPin-Priority: 990\n' > /etc/apt/preferences.d/mister-target \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      gcc-10-arm-linux-gnueabihf g++-10-arm-linux-gnueabihf \
      binutils-arm-linux-gnueabihf make ca-certificates \
 && rm -rf /var/lib/apt/lists/*
