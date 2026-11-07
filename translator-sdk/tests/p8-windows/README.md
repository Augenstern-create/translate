# P8 Windows 证据（NAN-WINDOWS，commit `3cecd27`）

- 主机：NAN-WINDOWS，Windows 11 25H2，Rust 1.99.1，Python 3.13.5
- 工作区：`E:\augenstern\translate`；`CARGO_HOME`/`CARGO_TARGET_DIR`/`TEMP`/`ORT_CACHE_DIR`/`HF_HOME` 均指向工作区内
- 结果：默认 **354 passed / 0 failed / 60 ignored**；all-features **354 / 0 / 65**；fmt、clippy（默认+全 feature）、
  G08（1 passed）、ORT（13+6+5 passed）、license audit（172 包）通过；G06 为 0 tests（Linux 专属，N/A）；
  release gate 仍 FAIL，仅 `['G12','G11']`。
- 原始日志：Windows `.build\reports\P7-*`；本目录为 UTF-8 转存。

报告：`docs/49_P8_多实例共享资源与缓存一致性实施报告.md`。
