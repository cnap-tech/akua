// helm-engine-wasm — a Go wrapper around helm.sh/helm/v4/pkg/engine
// compiled to wasip1 and consumed from Rust via wasmtime.
//
// Exposes a small C-style ABI (same idiom as kcl.wasm):
//
//	helm_malloc(size i32) -> i32
//	helm_free(ptr i32)                // size ignored; we track it internally
//	helm_render(input_ptr i32, input_len i32) -> i32    // returns ptr to null-terminated JSON
//	helm_result_len(ptr i32) -> i32                     // length of result without NUL
//
// Input (JSON at input_ptr/input_len):
//
//	{
//	  "chart_tar_gz_b64": "<base64 tarball>",
//	  "values_yaml": "<yaml string>",
//	  "release": { "name": "...", "namespace": "...", "revision": 1 }
//	}
//
// Output (C-string null-terminated):
//
//	{
//	  "manifests": { "<path>": "<yaml>" },
//	  "error": ""
//	}
//
// No Extism — plain wasmtime host. The Go runtime + klog init chain need
// more WASI surface than Extism's deny-all exposes. The sandbox posture
// is enforced one layer up: wasmtime host grants no preopens, no network,
// dummy argv only. See `../src/lib.rs` docstring.

package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"unsafe"

	ci "helm.sh/helm/v4/pkg/chart"
	"helm.sh/helm/v4/pkg/chart/common"
	"helm.sh/helm/v4/pkg/chart/common/util"
	"helm.sh/helm/v4/pkg/chart/v2/loader"
	"helm.sh/helm/v4/pkg/engine"
	"sigs.k8s.io/yaml"
)

// main is the wasip1 entrypoint. Runs once to initialise the Go runtime
// (and all package init() chains — klog, k8s.io/..., helm) then parks.
// The host calls our exports afterwards.
func main() {}

// allocations keeps live buffers reachable so the GC doesn't collect them
// between host calls.
var allocations = map[uintptr][]byte{}

//go:wasmexport helm_malloc
func helmMalloc(size int32) int32 {
	if size <= 0 {
		return 0
	}
	buf := make([]byte, size)
	ptr := uintptr(unsafe.Pointer(&buf[0]))
	allocations[ptr] = buf
	return int32(ptr)
}

//go:wasmexport helm_free
func helmFree(ptr int32) {
	delete(allocations, uintptr(ptr))
}

//go:wasmexport helm_result_len
func helmResultLen(ptr int32) int32 {
	buf, ok := allocations[uintptr(ptr)]
	if !ok {
		return 0
	}
	// Find NUL terminator
	for i, b := range buf {
		if b == 0 {
			return int32(i)
		}
	}
	return int32(len(buf))
}

type renderInput struct {
	ChartTarGzBase64 string      `json:"chart_tar_gz_b64"`
	ValuesYAML       string      `json:"values_yaml"`
	Release          releaseInfo `json:"release"`
}

type releaseInfo struct {
	Name      string `json:"name"`
	Namespace string `json:"namespace"`
	Revision  int    `json:"revision"`
	Service   string `json:"service"`
}

type renderOutput struct {
	Manifests map[string]string `json:"manifests,omitempty"`
	Error     string            `json:"error,omitempty"`
}

//go:wasmexport helm_render
func helmRender(inputPtr int32, inputLen int32) int32 {
	input := readBytes(inputPtr, inputLen)
	return writeResult(renderInternal(input))
}

func renderInternal(inputBytes []byte) renderOutput {
	var in renderInput
	if err := json.Unmarshal(inputBytes, &in); err != nil {
		return renderOutput{Error: fmt.Sprintf("parsing input JSON: %s", err)}
	}
	tarGz, err := base64.StdEncoding.DecodeString(in.ChartTarGzBase64)
	if err != nil {
		return renderOutput{Error: fmt.Sprintf("decoding chart_tar_gz_b64: %s", err)}
	}
	ch, err := loader.LoadArchive(bytes.NewReader(tarGz))
	if err != nil {
		return renderOutput{Error: fmt.Sprintf("loading chart archive: %s", err)}
	}
	values, err := parseValues(in.ValuesYAML)
	if err != nil {
		return renderOutput{Error: fmt.Sprintf("parsing values YAML: %s", err)}
	}
	// util.ToRenderValues performs the chart-hierarchy value coalescing
	// (subchart defaults, alias scoping) that engine.Render relies on.
	releaseOpts := common.ReleaseOptions{
		Name:      in.Release.Name,
		Namespace: in.Release.Namespace,
		Revision:  in.Release.Revision,
		IsInstall: true,
		IsUpgrade: false,
	}
	if releaseOpts.Name == "" {
		releaseOpts.Name = "release"
	}
	if releaseOpts.Namespace == "" {
		releaseOpts.Namespace = "default"
	}
	if releaseOpts.Revision == 0 {
		releaseOpts.Revision = 1
	}
	renderScope, err := util.ToRenderValues(ci.Charter(ch), values, releaseOpts, nil)
	if err != nil {
		return renderOutput{Error: fmt.Sprintf("preparing render values: %s", err)}
	}
	rendered, err := engine.Render(ci.Charter(ch), renderScope)
	if err != nil {
		return renderOutput{Error: fmt.Sprintf("helm engine: %s", err)}
	}
	return renderOutput{Manifests: rendered}
}

func parseValues(yamlStr string) (map[string]any, error) {
	if yamlStr == "" {
		return map[string]any{}, nil
	}
	out := map[string]any{}
	if err := yaml.Unmarshal([]byte(yamlStr), &out); err != nil {
		return nil, err
	}
	return out, nil
}

func readBytes(ptr int32, length int32) []byte {
	// SAFETY: wasmtime passes linear-memory pointers valid for the call.
	return unsafe.Slice((*byte)(unsafe.Pointer(uintptr(ptr))), length)
}

// writeResult marshals the render result to a NUL-terminated JSON buffer,
// stores it in the allocation table, returns the pointer.
func writeResult(out renderOutput) int32 {
	data, err := json.Marshal(out)
	if err != nil {
		data = []byte(fmt.Sprintf(`{"error":"marshal: %s"}`, err.Error()))
	}
	buf := make([]byte, len(data)+1)
	copy(buf, data)
	buf[len(data)] = 0
	ptr := uintptr(unsafe.Pointer(&buf[0]))
	allocations[ptr] = buf
	return int32(ptr)
}
