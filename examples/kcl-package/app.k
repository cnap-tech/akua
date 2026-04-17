# Minimal KCL program producing a Kubernetes ConfigMap.
# Akua's KCL engine runs `kcl run` on this file and captures the YAML output.

apiVersion = "v1"
kind = "ConfigMap"
metadata = {
    name = "hello-kcl"
    namespace = "default"
    labels = {
        "app.kubernetes.io/name" = "hello-kcl"
        "akua.tech/engine" = "kcl"
    }
}
data = {
    "greeting" = "Hello from KCL, compiled by Akua into a Helm chart."
    "rendered-at" = "build-time"
}
