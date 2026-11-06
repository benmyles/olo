# Face unlock's models

- `yunet.onnx`: YuNet, OpenCV Zoo's face detector (`face_detection_yunet_2023mar.onnx`),
  by Shiqi Yu and others; MIT licence (`LICENSE-yunet`).
- `sface.onnx`: SFace, OpenCV Zoo's face recogniser (`face_recognition_sface_2021dec.onnx`);
  Apache 2.0 licence (`LICENSE-sface`).

From github.com/opencv/opencv_zoo (models/face_detection_yunet, models/face_recognition_sface),
unchanged. sha256: yunet 8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4,
sface 0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79.
Run in process by `olo-face` (tract); see HACKING's "Face unlock".
