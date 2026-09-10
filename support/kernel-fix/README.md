# Optional MiSTer exFAT kernel fix

This optional package replaces one specific MiSTer Linux kernel to improve exFAT directory scanning. It is separate from Degauss, its installer and its updater. It does not change games, cores, settings, the filesystem, `linux.img`, modules, the bootloader or Update All configuration.

The replacement recognises only distribution `260907`, Linux `6.18.38-MiSTer`, the exact original kernel and this exact replacement. Unknown, modified and newer kernels are refused. This is a source rebuild with the exFAT fix and the original device tree, not an in-place patch of the official binary or a universal kernel replacement.

## Install or restore

The [optional ZIP](mister-exfat-fix-260907.zip) contains exactly four files: two Scripts entries, their shared helper, and one kernel. Nothing else needs copying to the card.

1. Back up the card and keep a card reader available. Use stable power. Do not run Update All, Downloader or another kernel updater at the same time.
2. Extract the ZIP. Copy these three items from its `Scripts` folder into the card's existing `Scripts` folder, keeping their names unchanged:

   - `Install_exFAT_Fix.sh`
   - `Restore_Original_Kernel.sh`
   - `.degauss-exfat-fix`, the whole hidden folder containing the other two files. Enable “show hidden files” on your computer if needed.

   Keep everything already in the card's `Scripts` folder.
3. In MiSTer's Scripts menu, run `Install_exFAT_Fix`. A keyboard is needed. Type `y` and press Enter to install, or press Enter to cancel.
4. Wait for **Verified replacement complete**. The script never reboots automatically. Follow the terminal prompts to return to MiSTer.
5. Before rebooting, keep an off-card copy of `linux/degauss-exfat-original-260907.zImage_dtb`, then reboot when ready.

To undo the change, run `Restore_Original_Kernel`, confirm with `y`, wait for verified completion, and reboot. The original backup is verified and never overwritten or removed. Repeating Install or Restore when the requested kernel is already installed makes no changes. A fix installed outside this package without an original backup produces an explicit warning; Restore cannot manufacture the missing original.

This package leaves the Linux release marker and normal update configuration unchanged. A later official update may replace it. Do not force this old kernel or backup onto a newer installation.

## If a write fails or MiSTer will not boot

Read the error before rebooting. A write or verification failure is not success. An incomplete or unknown backup is refused rather than overwritten. No installer can guarantee recovery from a power failure.

For offline recovery on the unchanged `260907` installation, power off MiSTer and put the card in a computer. Verify the original backup against the checksum below, then **copy**, do not move, it over `linux/zImage_dtb`. Replace only that file, keep the backup, and eject the card safely. If the backup is missing or damaged, or the Linux installation has changed, stop rather than choosing an arbitrary replacement.

| Artifact | SHA-256 |
| --- | --- |
| Optional ZIP | `65e0b8f5a80a9dccc994633e6b9f8c6e57d3f5d4619ce7313d2d9e8cdc1786da` |
| Recognised original kernel | `0bec449e365e757d14711bead76c5b1805d2b613a39e633fe7dc9f08e5687e73` |
| Included replacement kernel | `b350283e0aa306cc24ef2a2e8213cd11ae1345054059239cb1516a81f9d5c706` |

The original kernel is not included: it is backed up from the recognised installation. Installation and restoration do not download anything.

## Source and licences

The three installer scripts are plain Bash source inside the ZIP and use Degauss's existing [PolyForm Noncommercial licence](../../LICENSE). The Linux kernel is separate and retains its [upstream COPYING terms](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/COPYING), including [GPL-2.0](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/LICENSES/preferred/GPL-2.0), the [Linux syscall note](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/LICENSES/exceptions/Linux-syscall-note) and its component notices. Degauss's noncommercial restrictions do not apply to Linux.

The exact source base is [MiSTer Linux commit aec7dc3aa4846385736f1d54c9155e3b3c726708](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/tree/aec7dc3aa4846385736f1d54c9155e3b3c726708), available as a [complete source archive](https://codeload.github.com/MiSTer-devel/Linux-Kernel_MiSTer/tar.gz/aec7dc3aa4846385736f1d54c9155e3b3c726708). Apply only the four-line change from [the merged upstream fix](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/commit/9854075c86455942c2ce57e0b7dc80e3e2c5b108), also available as a [patch](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/commit/9854075c86455942c2ce57e0b7dc80e3e2c5b108.patch), to that base. The merged commit's entire tree contains other changes and is not the source base of this binary.

The exact build configuration is embedded in the included kernel. The pinned source's [`scripts/extract-ikconfig`](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/scripts/extract-ikconfig) recovers it. The board source is [`socfpga_cyclone5_de10_nano.dts`](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/arch/arm/boot/dts/intel/socfpga/socfpga_cyclone5_de10_nano.dts), not the similarly named `de10nano` variant. Its includes and notices are in the same complete source tree. No separate source fork or source-tree copy is required by these reconstruction steps.

<details>
<summary>Reconstruction details for developers</summary>

These are workstation instructions, not card-install commands. Keep the complete source tree and its notices. The archive checksum is `f3710785997f6add93ee1efe1cd3828f498f8eefdcf1a6136790d8db598e9e3d`. Apply the linked patch to the pinned base with `git apply --check` followed by `git apply`. The resulting `fs/exfat/dir.c` Git blob is `e6dc24a956c7f7bae085577ba8a2de1a7c1132cc`.

From the patched source directory, with the ZIP extracted in the parent directory:

```sh
mkdir ../build
LC_ALL=C sh scripts/extract-ikconfig \
  ../Scripts/.degauss-exfat-fix/zImage_dtb > ../build/.config
```

The recovered configuration's SHA-256 must be `dc57d44a17c3b2108520af03f76da5622c34baee1103324a610a13ba64384004`.

The build used Debian GCC `arm-linux-gnueabihf-gcc-10` 10.2.1-6, GNU binutils 2.40 and host GCC. Install the matching `gcc-10-plugin-dev-arm-linux-gnueabihf` headers, LZ4 and the kernel's usual build dependencies, including make, bc, bison, flex, cpio, OpenSSL and libelf development headers. See the pinned source's [build requirements](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/aec7dc3aa4846385736f1d54c9155e3b3c726708/Documentation/process/changes.rst).

```sh
make O=../build ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- \
  CC=arm-linux-gnueabihf-gcc-10 HOSTCC=gcc olddefconfig
make O=../build ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- \
  CC=arm-linux-gnueabihf-gcc-10 HOSTCC=gcc LOCALVERSION=-MiSTer \
  KBUILD_BUILD_USER=kernel-test KBUILD_BUILD_HOST=local \
  KBUILD_BUILD_VERSION=103 -j8 zImage
```

Inspect configuration changes instead of silently accepting dropped features. The appended device tree can be built from the source-tree root with these preprocessing and DTC 1.7.2 options:

```sh
clang -E -P -x assembler-with-cpp -nostdinc -undef -D__DTS__ \
  -I include \
  arch/arm/boot/dts/intel/socfpga/socfpga_cyclone5_de10_nano.dts \
  -o de10-nano.preprocessed.dts
dtc -I dts -O dtb -o de10-nano.dtb de10-nano.preprocessed.dts
cat ../build/arch/arm/boot/zImage de10-nano.dtb > ../build/zImage_dtb
```

The DTB is 20,229 bytes, SHA-256 `315ec64aa3f661c2359a83cdf1cd601cf8cc9ddb20214315f36c81520e21a009`. It is appended directly after the zImage, without a header or padding, and matches the original kernel's device tree.

Different toolchains, build timestamps and environments can change kernel bytes. These instructions identify the source and build inputs; they do not promise a byte-identical arbitrary-host rebuild. The guarded installer recognises only the included, separately validated artifact, not another kernel produced from these instructions.

</details>
