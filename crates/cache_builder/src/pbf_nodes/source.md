# source.rs

Separates live mount identity from durable resume evidence. Stamp equality stays
strict (device, inode, length, nanosecond mtime) during an active build. Resume
permits only device-number changes; inode, length and mtime must still match.
macOS can assign a new st_dev to the same external volume after a reboot/remount.

A versioned SHA-256 fingerprint samples 64 evenly spaced 64 KiB source windows,
including the header and tail. It adds bounded content evidence on each open
without rescanning a 90+ GB planet. It is not a full-file checksum; adversarial
changes outside samples with restored metadata remain outside this contract.
Both the opened descriptor and source path are checked before and after sampling.

Legacy checkpoints have no fingerprint. index.rs validates representative saved
block descriptors against the source before upgrading them in place. Errors
preserve existing checkpoint data and explain which identity fields changed.
