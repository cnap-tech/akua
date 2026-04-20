# PolicySet — a control-plane KRM kind. Typed KCL; not YAML.
#
# Composes imported tiers with the local workspace rego package into one
# PolicySet that environments reference. Each environment picks which
# PolicySet it uses via `Environment.spec.policy`.
#
# YAML is a derived view only:  akua export policy-set.k --format=yaml

import akua.krm.policy_set as ps

my_org_production = ps.PolicySet {
    metadata = {
        name: "my-org-production"
        labels.tier: "production"
    }

    spec = ps.Spec {
        # Sources composed into this set, in order. Later sources can
        # override earlier ones by emitting a rule with the same name.
        sources = [
            # The reference tier (signed OCI artifact, pinned via akua.mod).
            ps.Source {
                kind = "OciBundle"
                ref  = "oci://policies.akua.dev/tier/production@sha256:9f4cc2e0..."
            }
            # Kyverno bundle, converted to Rego at `akua add` time.
            ps.Source {
                kind = "OciBundle"
                ref  = "oci://policies.akua.dev/kyverno/security@sha256:1a2b3c4d..."
            }
            # Local Rego package — the workspace-specific rules.
            ps.Source {
                kind = "LocalRego"
                path = "./policies/"
            }
        ]

        # Default verdict when a rule produces `needs-approval` instead of
        # `deny`. Humans fill the gap through the review surface.
        on_needs_approval = {
            reviewers = ["@team/platform", "@team/payments-lead"]
            timeout   = "24h"
        }

        # Minimum akua version that can evaluate this set. Protects the
        # gatekeeper from silently behaving differently across versions.
        min_akua_version = "0.1.0"
    }
}
