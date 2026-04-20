package akua.policies.workspace_test

import data.akua.policies.workspace

# --- fixtures ---

good_deployment = {
    "apiVersion": "apps/v1",
    "kind": "Deployment",
    "metadata": {
        "name": "checkout",
        "labels": {"team": "payments"},
    },
    "spec": {"replicas": 5},
}

deployment_missing_team = {
    "apiVersion": "apps/v1",
    "kind": "Deployment",
    "metadata": {"name": "checkout"},
    "spec": {"replicas": 5},
}

deployment_prod_too_few_replicas = {
    "apiVersion": "apps/v1",
    "kind": "Deployment",
    "metadata": {"name": "checkout", "labels": {"team": "payments"}},
    "spec": {"replicas": 1},
}

prod_env = {"name": "production"}

# --- tests ---

test_good_deployment_allows {
    count(workspace.deny) == 0 with input as {
        "resource":    good_deployment,
        "environment": prod_env,
    }
}

test_missing_team_denies {
    result := workspace.deny with input as {
        "resource":    deployment_missing_team,
        "environment": prod_env,
    }
    some msg in result
    contains(msg, "must have a metadata.labels.team")
}

test_prod_too_few_replicas_denies {
    result := workspace.deny with input as {
        "resource":    deployment_prod_too_few_replicas,
        "environment": prod_env,
    }
    some msg in result
    contains(msg, "replicas >= 2")
}
