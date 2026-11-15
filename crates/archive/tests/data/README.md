Sample RAR archives from the `unrar` crate's own tests (MIT/Apache-2.0,
https://github.com/muja/unrar.rs/tree/master/data): RAR files can only
be made by RARLAB's own program, so these are read, not made, in tests.
`crypted.rar`'s password is "unrar".

UDF images, kept compressed with zstd (each is mostly zeros), made with
real tools since that's what the reader has to agree with:
`genudf.iso` by genisoimage 1.1.11 (`-udf`: UDF 1.02 beside an ISO 9660
tree without Rock Ridge) from a folder of the files the test names;
the others empty, by mkudffs 2.3: `u102.img` UDF 1.02 in 512-byte
blocks with strategy 4096 (`--media-type=hd --udfrev=1.02
--strategy=4096`), `u201.img` 2.01 with the root's data in its entry
(`--media-type=dvd --ad=inicb`), `u201s.img` 2.01 in 512-byte blocks,
`u250.img` 2.50 as a BD-R has it (`--media-type=bdr`: a virtual
allocation table and a metadata partition).
