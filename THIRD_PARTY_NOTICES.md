# Third-party notices

The Utter installer bundles the following software and models. Each keeps its own license.

| Component | Use | License | Source |
| --- | --- | --- | --- |
| whisper.cpp (`whisper-server.exe` + DLLs) | Speech to text | MIT | https://github.com/ggml-org/whisper.cpp |
| llama.cpp (`llama-server.exe` + DLLs) | AI cleanup | MIT | https://github.com/ggml-org/llama.cpp |
| Whisper `tiny.en` and `base.en` models (ggml, q5_1) | Speech model weights | MIT (OpenAI) | https://huggingface.co/ggerganov/whisper.cpp |
| Qwen2.5-0.5B-Instruct (GGUF, Q4_K_M) | Cleanup model weights | Apache License 2.0 (Alibaba Cloud) | https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF |
| Tauri and its Rust crates | App shell | MIT / Apache-2.0 | https://tauri.app |
| LLVM OpenMP runtime (`libomp.dll`, shipped with llama.cpp) | Threading | Apache-2.0 with LLVM exception | https://openmp.llvm.org |

Qwen2.5 is used unmodified under the Apache License 2.0: https://www.apache.org/licenses/LICENSE-2.0
