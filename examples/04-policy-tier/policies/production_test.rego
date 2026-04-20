# production_test.rego — tests for the local rules in production.rego.
#
# Run with:  akua test policies/
#
# File naming convention per cli.md: `*_test.rego`. Rules named `test_*` are
# discovered and executed by the embedded OPA test runner.

package akua.policies.my_org_production_test

import data.akua.policies.my_org_production

# --- fixtures ---

good_deployment = {
    "apiVersion": "apps/v1",
    "kind": "Deployment",
    "metadata": {
        "name": "checkout",
        "labels": {"team": "payments"},
    },
    "spec": {"template": {"spec": {"containers": [
        {"resources": {"requests": {"cpu": "500m"}}},
    ]}}},
}

bad_deployment_missing_team = {
    "apiVersion": "apps/v1",
    "kind": "Deployment",
    "metadata": {"name": "checkout"},
    "spec": {"template": {"spec": {"containers": [
        {"resources": {"requests": {"cpu": "500m"}}},
    ]}}},
}

prod_env = {
    "name": "production",
    "budget": {"cpu_millicores": 2000},
}

# --- tests ---

test_good_deployment_allows {
    count(my_org_production.deny) == 0 with input as {
        "resource":     good_deployment,
        "resources":    [good_deployment],
        "environment":  prod_env,
    }
}

test_missing_team_denies {
    result := my_org_production.deny with input as {
        "resource":     bad_deployment_missing_team,
        "resources":    [bad_deployment_missing_team],
        "environment":  prod_env,
    }
    some msg in result
    contains(msg, "must have a metadata.labels.team")
}

test_budget_overrun_denies {
    big := {
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {"name": "fat", "labels": {"team": "t"}},
        "spec": {"template": {"spec": {"containers": [
            {"resources": {"requests": {"cpu": "5000m"}}},
        ]}}},
    }
    result := my_org_production.deny with input as {
        "resource":     big,
        "resources":    [big],
        "environment":  prod_env,
    }
    some msg in result
    contains(msg, "exceed environment budget")
}

# CPU parsing helper tests.
test_cpu_millicores_mill { my_org_production.cpu_millicores("500m") == 500 }
test_cpu_millicores_whole { my_org_production.cpu_millicores("2") == 2000 }
