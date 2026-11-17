# The models that read the text in a picture

`olo-ocr` (crates/ocr) reads the text in a picture with Robert Knight's
[Ocrs](https://github.com/robertknight/ocrs): a model that finds where
the words are and a model that reads each line, run in process by his
RTen. These are its two pre-trained models, as its own
`examples/download-models.sh` fetches them:

| File | From | SHA-256 |
|---|---|---|
| `text-detection.onnx` | `https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx` | `a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe` |
| `text-recognition.onnx` | `https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx` | `86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5` |

They read Latin letters, digits and punctuation. Trained by Robert
Knight on [HierText](https://github.com/google-research-datasets/hiertext)
and synthetic data ([ocrs-models](https://github.com/robertknight/ocrs-models));
licensed **CC BY-SA 4.0** (`LICENSE`), as their model page
(https://huggingface.co/robertknight/ocrs) says. They are shipped
unchanged, as data beside Olo's own code (which is under Olo's licence),
and installed to `share/olo/ocr`.
