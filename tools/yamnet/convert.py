#!/usr/bin/env python3
"""Build ``yamnet.onnx`` from Google's official YAMNet Keras weights.

YAMNet is Apache-2.0 (https://github.com/tensorflow/models/tree/master/research/audioset/yamnet).
The network is a MobileNetV1 stack, so the ONNX graph is written directly from
the weights instead of going through TensorFlow and tf2onnx. The output is
deterministic for a given ``onnx`` version, which lets ``cargo xtask models``
pin its hash.

Input ``patches``: float32 [N, 96, 64] log-mel patches (see
``scripts/tier2_lib.py::yamnet_patches``). Outputs ``scores`` [N, 521]
(per-class sigmoid) and ``embeddings`` [N, 1024].

Usage: python convert.py yamnet.h5 yamnet.onnx
"""

from __future__ import annotations

import sys

import h5py
import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

PATCH_FRAMES = 96
MEL_BANDS = 64
NUM_CLASSES = 521
BATCHNORM_EPSILON = 1e-4

# (stride, filters) per layer, from yamnet.py `_YAMNET_LAYER_DEFS`. Layer 1 is a
# full 3x3 convolution; the rest are depthwise-separable.
LAYERS = [
    (2, 32),
    (1, 64),
    (2, 128),
    (1, 128),
    (2, 256),
    (1, 256),
    (2, 512),
    (1, 512),
    (1, 512),
    (1, 512),
    (1, 512),
    (1, 512),
    (2, 1024),
    (1, 1024),
]


class Graph:
    def __init__(self, weights: h5py.File) -> None:
        self.weights = weights
        self.nodes: list[onnx.NodeProto] = []
        self.initializers: list[onnx.TensorProto] = []

    def tensor(self, name: str) -> np.ndarray:
        group, _, leaf = name.rpartition("/")
        return np.asarray(self.weights[group][f"{group}/{leaf}:0"], dtype=np.float32)

    def constant(self, name: str, value: np.ndarray) -> str:
        self.initializers.append(numpy_helper.from_array(np.ascontiguousarray(value), name))
        return name

    def node(self, op: str, inputs: list[str], output: str, **attrs) -> str:
        self.nodes.append(helper.make_node(op, inputs, [output], name=output, **attrs))
        return output

    def conv_bn_relu(self, x: str, prefix: str, kernel: np.ndarray, stride: int, group: int) -> str:
        weight = self.constant(f"{prefix}/kernel", kernel)
        x = self.node(
            "Conv",
            [x, weight],
            f"{prefix}/conv",
            strides=[stride, stride],
            auto_pad="SAME_UPPER",  # TensorFlow "same" padding
            group=group,
        )
        beta = self.tensor(f"{prefix}/bn/beta")
        x = self.node(
            "BatchNormalization",
            [
                x,
                self.constant(f"{prefix}/bn/scale", np.ones_like(beta)),  # scale=False
                self.constant(f"{prefix}/bn/beta", beta),
                self.constant(f"{prefix}/bn/mean", self.tensor(f"{prefix}/bn/moving_mean")),
                self.constant(f"{prefix}/bn/var", self.tensor(f"{prefix}/bn/moving_variance")),
            ],
            f"{prefix}/bn",
            epsilon=BATCHNORM_EPSILON,
        )
        return self.node("Relu", [x], f"{prefix}/relu")


def build(weights: h5py.File) -> onnx.ModelProto:
    graph = Graph(weights)
    x = graph.node(
        "Unsqueeze",
        ["patches", graph.constant("channel_axis", np.array([1], dtype=np.int64))],
        "input",
    )
    for index, (stride, _filters) in enumerate(LAYERS, start=1):
        layer = f"layer{index}"
        if index == 1:
            # Keras [kh, kw, in, out] -> ONNX [out, in, kh, kw]
            kernel = graph.tensor(f"{layer}/conv/kernel").transpose(3, 2, 0, 1)
            x = graph.conv_bn_relu(x, f"{layer}/conv", kernel, stride, group=1)
            continue
        # Depthwise [kh, kw, in, 1] -> [in, 1, kh, kw], one group per channel.
        depthwise = graph.tensor(f"{layer}/depthwise_conv/depthwise_kernel").transpose(2, 3, 0, 1)
        x = graph.conv_bn_relu(
            x, f"{layer}/depthwise_conv", depthwise, stride, group=depthwise.shape[0]
        )
        pointwise = graph.tensor(f"{layer}/pointwise_conv/kernel").transpose(3, 2, 0, 1)
        x = graph.conv_bn_relu(x, f"{layer}/pointwise_conv", pointwise, 1, group=1)

    pooled = graph.node("GlobalAveragePool", [x], "pooled")
    embeddings = graph.node("Flatten", [pooled], "embeddings", axis=1)
    logits = graph.node(
        "MatMul", [embeddings, graph.constant("logits/kernel", graph.tensor("logits/kernel"))], "logits/matmul"
    )
    logits = graph.node(
        "Add", [logits, graph.constant("logits/bias", graph.tensor("logits/bias"))], "logits"
    )
    graph.node("Sigmoid", [logits], "scores")

    onnx_graph = helper.make_graph(
        graph.nodes,
        "yamnet",
        [helper.make_tensor_value_info("patches", TensorProto.FLOAT, ["N", PATCH_FRAMES, MEL_BANDS])],
        [
            helper.make_tensor_value_info("scores", TensorProto.FLOAT, ["N", NUM_CLASSES]),
            helper.make_tensor_value_info("embeddings", TensorProto.FLOAT, ["N", 1024]),
        ],
        graph.initializers,
    )
    model = helper.make_model(
        onnx_graph,
        producer_name="tundra-yamnet-convert",
        opset_imports=[helper.make_opsetid("", 17)],
        doc_string="YAMNet (c) 2019 The TensorFlow Authors, Apache License 2.0. Converted to ONNX.",
    )
    model.ir_version = 9
    onnx.checker.check_model(model)
    return model


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit("usage: convert.py yamnet.h5 yamnet.onnx")
    with h5py.File(sys.argv[1], "r") as weights:
        model = build(weights)
    onnx.save_model(model, sys.argv[2])


if __name__ == "__main__":
    main()
