# Model notices

## yamnet.onnx

Converted from [YAMNet](https://github.com/tensorflow/models/tree/master/research/audioset/yamnet)
(`yamnet.h5`), Copyright 2019 The TensorFlow Authors, licensed under the
Apache License, Version 2.0 (see `LICENSE`).

Modification: the Keras weights were converted to ONNX, unchanged, by
`tools/yamnet/convert.py`. `tools/yamnet/verify.py` checks the result against
the TensorFlow reference implementation.

## yamnet_class_map.csv

From the same YAMNet release (tensorflow/models commit
`c14bf9ad91962cf189f9f58db2132c06247fcd53`), unmodified. The class names are
from the [AudioSet ontology](https://research.google.com/audioset/ontology/index.html)
by Google, licensed under
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).
