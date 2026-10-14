"""Local OpenAI-compatible speech-to-text server for UnoFlow.

Lets UnoFlow run fully offline: no OpenAI key, no audio leaving the machine.

It exposes the same endpoint UnoFlow already calls:
    POST /v1/audio/transcriptions   (multipart: file, model, language, prompt)

so you only change one field in UnoFlow's Settings:
    API base URL = http://127.0.0.1:8000/v1
and leave the API key blank (an empty key is allowed for non-OpenAI hosts).

Backend is faster-whisper (CTranslate2), which ships as wheels -- no compiler,
no CUDA toolkit needed. Defaults to CPU int8, which is the safe choice on a
3 GB GPU; see README for the GPU/CPU trade-off.

Run:
    python server.py
    # or pick a model/device:
    set UNOFLOW_STT_MODEL=small.en
    set UNOFLOW_STT_DEVICE=cuda
    python server.py
"""

from __future__ import annotations

import os
import tempfile

from fastapi import FastAPI, File, Form, UploadFile
from faster_whisper import WhisperModel

MODEL_NAME = os.environ.get("UNOFLOW_STT_MODEL", "base.en")
DEVICE = os.environ.get("UNOFLOW_STT_DEVICE", "cpu")
HOST = os.environ.get("UNOFLOW_STT_HOST", "127.0.0.1")
PORT = int(os.environ.get("UNOFLOW_STT_PORT", "8000"))
COMPUTE_TYPE = os.environ.get(
    "UNOFLOW_STT_COMPUTE", "int8" if DEVICE == "cpu" else "float16"
)
# Use VAD to skip silence, which matters for push-to-talk clips that start or
# end with a pause.
USE_VAD = os.environ.get("UNOFLOW_STT_VAD", "1") != "0"

app = FastAPI(title="UnoFlow local STT")
_model: WhisperModel | None = None


def get_model() -> WhisperModel:
    """Load the model once, on first request."""
    global _model
    if _model is None:
        print(f"[stt] loading {MODEL_NAME} ({DEVICE}/{COMPUTE_TYPE})…", flush=True)
        _model = WhisperModel(MODEL_NAME, device=DEVICE, compute_type=COMPUTE_TYPE)
        print("[stt] ready", flush=True)
    return _model


@app.get("/health")
def health() -> dict:
    return {"ok": True, "model": MODEL_NAME, "device": DEVICE, "compute": COMPUTE_TYPE}


@app.post("/v1/audio/transcriptions")
async def transcribe(
    file: UploadFile = File(...),
    # UnoFlow sends `model`; we ignore its value and use the server's model.
    model: str = Form(default=""),
    language: str = Form(default=""),
    prompt: str = Form(default=""),
) -> dict:
    data = await file.read()
    if not data:
        return {"text": ""}

    # faster-whisper reads from a path, so spool the upload to a temp file.
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
        tmp.write(data)
        path = tmp.name

    try:
        segments, _info = get_model().transcribe(
            path,
            language=(language or None),
            initial_prompt=(prompt or None),
            vad_filter=USE_VAD,
        )
        text = "".join(seg.text for seg in segments).strip()
        return {"text": text}
    finally:
        os.unlink(path)


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host=HOST, port=PORT, log_level="info")
