// kustomize-engine-wasm — a Go wrapper around sigs.k8s.io/kustomize
// compiled to wasip1 and consumed from Rust via wasmtime.
//
// Same ABI shape as helm-engine-wasm:
//
//	kustomize_malloc(size i32) -> i32
//	kustomize_free(ptr i32)
//	kustomize_build(input_ptr i32, input_len i32) -> i32    // returns ptr to NUL-terminated JSON
//	kustomize_result_len(ptr i32) -> i32
//
// Input (JSON at input_ptr/input_len):
//
//	{
//	  "overlay_tar_gz_b64": "<base64 tarball of the overlay dir tree>",
//	  "entrypoint": "overlay"           // root dir inside the tarball containing kustomization.yaml
//	}
//
// Output (C-string NUL-terminated):
//
//	{
//	  "yaml": "<rendered multi-doc yaml>",
//	  "error": ""
//	}
//
// Sandbox posture matches helm-engine-wasm: no filesystem preopens — we
// unpack the overlay tarball into kustomize's `filesys.FileSystem` in
// memory so the guest only sees what the host handed it.

package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"path"
	"unsafe"

	"sigs.k8s.io/kustomize/api/krusty"
	"sigs.k8s.io/kustomize/kyaml/filesys"
)

func main() {}

var allocations = map[uintptr][]byte{}

//go:wasmexport kustomize_malloc
func kustomizeMalloc(size int32) int32 {
	if size <= 0 {
		return 0
	}
	buf := make([]byte, size)
	ptr := uintptr(unsafe.Pointer(&buf[0]))
	allocations[ptr] = buf
	return int32(ptr)
}

//go:wasmexport kustomize_free
func kustomizeFree(ptr int32) {
	delete(allocations, uintptr(ptr))
}

//go:wasmexport kustomize_result_len
func kustomizeResultLen(ptr int32) int32 {
	buf, ok := allocations[uintptr(ptr)]
	if !ok {
		return 0
	}
	for i, b := range buf {
		if b == 0 {
			return int32(i)
		}
	}
	return int32(len(buf))
}

type buildInput struct {
	OverlayTarGzBase64 string `json:"overlay_tar_gz_b64"`
	Entrypoint         string `json:"entrypoint"`
}

type buildOutput struct {
	YAML  string `json:"yaml,omitempty"`
	Error string `json:"error,omitempty"`
}

//go:wasmexport kustomize_build
func kustomizeBuild(inputPtr int32, inputLen int32) int32 {
	input := readBytes(inputPtr, inputLen)
	return writeResult(buildInternal(input))
}

func buildInternal(inputBytes []byte) buildOutput {
	var in buildInput
	if err := json.Unmarshal(inputBytes, &in); err != nil {
		return buildOutput{Error: fmt.Sprintf("parsing input JSON: %s", err)}
	}
	tarGz, err := base64.StdEncoding.DecodeString(in.OverlayTarGzBase64)
	if err != nil {
		return buildOutput{Error: fmt.Sprintf("decoding overlay_tar_gz_b64: %s", err)}
	}

	fs := filesys.MakeFsInMemory()
	if err := unpackTarGzInto(fs, tarGz); err != nil {
		return buildOutput{Error: fmt.Sprintf("unpacking overlay tarball: %s", err)}
	}

	entrypoint := in.Entrypoint
	if entrypoint == "" {
		entrypoint = "overlay"
	}
	k := krusty.MakeKustomizer(krusty.MakeDefaultOptions())
	resMap, err := k.Run(fs, entrypoint)
	if err != nil {
		return buildOutput{Error: fmt.Sprintf("kustomize build: %s", err)}
	}
	yaml, err := resMap.AsYaml()
	if err != nil {
		return buildOutput{Error: fmt.Sprintf("resmap to yaml: %s", err)}
	}
	return buildOutput{YAML: string(yaml)}
}

// unpackTarGzInto writes each regular file in the tar.gz stream to `fs`
// under the path encoded in its tar header. Directories are created
// implicitly via fs.MkdirAll. Anything non-regular (symlink, device)
// is rejected — kustomize overlays shouldn't contain them, and we
// don't want to execute a guest-supplied symlink escape inside the
// in-memory fs.
func unpackTarGzInto(fs filesys.FileSystem, data []byte) error {
	gz, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return fmt.Errorf("gzip: %w", err)
	}
	defer gz.Close()
	tr := tar.NewReader(gz)
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return fmt.Errorf("tar: %w", err)
		}
		switch hdr.Typeflag {
		case tar.TypeDir:
			if err := fs.MkdirAll(hdr.Name); err != nil {
				return fmt.Errorf("mkdir %s: %w", hdr.Name, err)
			}
		case tar.TypeReg, tar.TypeRegA:
			dir := path.Dir(hdr.Name)
			if dir != "" && dir != "." {
				if err := fs.MkdirAll(dir); err != nil {
					return fmt.Errorf("mkdir %s: %w", dir, err)
				}
			}
			body, err := io.ReadAll(tr)
			if err != nil {
				return fmt.Errorf("read %s: %w", hdr.Name, err)
			}
			if err := fs.WriteFile(hdr.Name, body); err != nil {
				return fmt.Errorf("write %s: %w", hdr.Name, err)
			}
		default:
			// Skip symlinks + device files + etc. — overlays don't need them.
		}
	}
}

func readBytes(ptr int32, length int32) []byte {
	return unsafe.Slice((*byte)(unsafe.Pointer(uintptr(ptr))), length)
}

func writeResult(out buildOutput) int32 {
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
