# Supabase AI module

This crate is part of the Supabase Edge Runtime stack and implements AI related
features for the `Supabase.ai` namespace.

## Model Execution Engine

`Supabase.ai` uses [onnxruntime](https://onnxruntime.ai/) as internal model
execution engine, backend by [ort pyke](https://ort.pyke.io/) rust bindings.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="/assets/docs/ai/onnx-backend-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="/assets/docs/ai/onnx-backend.svg">
    <img alt="ONNX Backend illustration" src="/assets/docs/ai/onnx-backend.svg" width="350" style="max-width: 100%;">
  </picture>
</p>

Following there's specific documentation for both "lands":

<details>
  <summary>Javascript/Frontend</summary>
</details>

<details>
  <summary>Rust/Backend</summary>
</details>

onnxruntime:

the Session class:
