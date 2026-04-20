package akua.policies.workspace_test

import data.akua.policies.workspace

# --- fixtures ---

good_app = {
    "kind": "App",
    "metadata": {
        "name": "checkout",
        "labels": {"team": "payments"},
    },
    "spec": {"inputs": {"production": {"replicas": 5}}},
}

app_missing_team = {
    "kind": "App",
    "metadata": {"name": "checkout"},
    "spec": {"inputs": {"production": {"replicas": 5}}},
}

app_prod_too_few_replicas = {
    "kind": "App",
    "metadata": {"name": "checkout", "labels": {"team": "payments"}},
    "spec": {"inputs": {"production": {"replicas": 1}}},
}

prod_env = {"name": "production"}

# --- tests ---

test_good_app_allows {
    count(workspace.deny) == 0 with input as {
        "resource":    good_app,
        "environment": prod_env,
    }
}

test_missing_team_denies {
    result := workspace.deny with input as {
        "resource":    app_missing_team,
        "environment": prod_env,
    }
    some msg in result
    contains(msg, "must have a metadata.labels.team")
}

test_prod_too_few_replicas_denies {
    result := workspace.deny with input as {
        "resource":    app_prod_too_few_replicas,
        "environment": prod_env,
    }
    some msg in result
    contains(msg, "replicas >= 2")
}
