# Framebuffer compatibility

MiSTer Linux 6.18's framebuffer driver does not supply the mapping callback
required by the kernel. Mapping `/dev/fb0` therefore returns `ENODEV`.
See the [kernel framebuffer entry point](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/33a0521fd46b3991ec3a882f659bceb2c1cb4399/drivers/video/fbdev/core/fb_chrdev.c)
and [MiSTer framebuffer driver](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/blob/33a0521fd46b3991ec3a882f659bceb2c1cb4399/drivers/video/fbdev/MiSTer_fb.c).

Degauss first tries the existing framebuffer mapping. Only `ENODEV` selects
the alternative `/dev/mem` mapping, using the physical address and capacity
reported by the driver. Other failures retain their original cause. A kernel
that restores native mapping automatically uses it again. The framebuffer
descriptor stays open for vertical synchronization.

Normal startup records the mapping source in `/tmp/degauss.log`. The existing
`--selftest` records it in `/tmp/degauss-selftest.log`. No new setting, Main
replacement, cache rebuild or settings reset is required for this fix.

## Acceptance checks

Use an isolated library containing a few known-working games. Preserve the
normal launcher, configuration and boot files before changing a test setup.
Kernel installation and reboot require separate device authorization.

1. On Linux 5.15, independently probe native framebuffer mapping and confirm
   it succeeds. Start the production ARM binary through Degauss Main and
   through the Scripts menu. Confirm the log reports native mapping.
2. Verify the first frame, input, list and artwork views, game launch, exit
   and return. Run `--selftest --frames 120` against the same isolated library
   and inspect both direct and staged presentation, including their timings.
3. On Linux 6.18, independently confirm native mapping returns `ENODEV`.
   Repeat the same launch, rendering, input, return and self-test checks;
   confirm the diagnostic identifies the `/dev/mem` mapping.
4. Exercise each connected output and geometry. Record which pixel format
   the driver actually reports. Host format tests do not establish an
   unavailable physical RGB565 or CRT output.
5. In a private mount namespace, make `/dev/mem` unmappable without changing
   global device permissions. Startup must fail with both the original
   framebuffer `ENODEV` and the failing `/dev/mem` operation. Restore the
   ordinary namespace and confirm recovery.
6. Restore the normal launcher and configuration and verify their original
   bytes. Confirm normal startup, saved controls and game return still work.

The surface tests cover native success without fallback, error selection,
physical range and alignment checks, overflow, both pixel formats, separate
pixel and unmap addresses, mapping cleanup and actionable failure causes.
Run the Linux-specific cases on Linux, including the ARM target. They do not
replace the two real-kernel checks above.
