# Sibling to the upstream nginx chart. Emits a ConfigMap that the web
# deployment's pods can mount at /etc/site — the nginx chart's values
# already support arbitrary configmap mounts, so the umbrella wires them
# together without any extra glue here.
#
# Eventually this file will pull values from the helm source via
# `${sources.web.values.replicaCount}`-style refs. That's deferred to
# v1alpha2; for now the two sources are independent.

apiVersion = "v1"
kind = "ConfigMap"
metadata = {
    name = "site-config"
    namespace = "default"
    labels = {
        "app.kubernetes.io/name" = "site-config"
        "app.kubernetes.io/part-of" = "hybrid-package"
        "akua.dev/engine" = "kcl"
    }
}
data = {
    "welcome.html" = "<h1>Served from an Akua hybrid package.</h1>"
    "build-metadata" = "engine=kcl; umbrella=hybrid-package"
}
