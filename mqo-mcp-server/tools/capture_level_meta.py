#!/usr/bin/env python3
"""Capture per-level domain/type metadata from a LIVE AtScale cluster.

This is the catalog-capture step for PRD-mqo-catalog-level-domain-metadata: it
probes each requested dimension level with one cheap `measure + level` query,
reads the dimension column to enumerate the member domain (when cardinality
<= CAP), and infers the value type from the members. The emitted `level_meta`
entries are merged into the served catalog so mqo-param-validator RULE 4
(filter-level guard) can decide value-fit instead of staying dormant.

The source the validator's static catalog lacks (search_columns / ColumnEntry
carry no level type or domain) IS available here: the live cluster answers a
bounded DISTINCT directly. Usage:

    ATSCALE_OIDC_SECRET=... python3 capture_level_meta.py > level_meta.json

Levels and the measure used to reach each fact are configured in TARGETS below
(a level is reachable only paired with a measure on a conformant fact). A probe
that errors (cross-fact, etc.) is skipped — capture is best-effort.
"""
import json, sys, subprocess, select, os, re

CAP = 1000  # cardinality ceiling: enumerate the domain at/below this, else descriptor
SRV = os.path.expanduser("~/.local/bin/mqo-mcp-server")
FIXTURE = os.path.expanduser(
    "~/projects/mqo-mcp/mqo-mcp-server/fixtures/tpcds_catalog.json"
)
LAUNCH = [
    SRV, "--catalog", FIXTURE,
    "--endpoint", "mcp-aws.atscaleinternal.com:15432",
    "--xmla-url", "https://mcp-aws.atscaleinternal.com/v1/xmla",
    "--oidc-token-url",
    "https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token",
    "--oidc-client-id", "atscale-mcp", "--oidc-realm", "atscale",
    "--oidc-client-secret-env", "ATSCALE_OIDC_SECRET",
]

# (hierarchy, level, reaching-measure). The measure only needs to share a fact
# with the level so the query binds; its values are ignored.
TARGETS = [
    ("store_dimension", "Store State Name", "Total Store Sales"),
    ("store_dimension", "Store State", "Total Store Sales"),
    ("store_dimension", "Store County", "Total Store Sales"),
    ("sold_date_dimensions", "Sold Day Name", "Total Store Sales"),
    ("sold_date_dimensions", "Sold Month Name", "Total Store Sales"),
    ("sold_date_dimensions", "Sold Quarter of Year", "Total Store Sales"),
    ("sold_date_dimensions", "Sold Month of Year", "Total Store Sales"),
    ("product_dimension", "Product Category", "Total Store Sales"),
    ("product_dimension", "Product Class Name", "Total Store Sales"),
    ("customer_demographics", "Marital Status", "Total Store Sales"),
    ("customer_demographics", "Gender", "Total Store Sales"),
    ("customer_demographics", "Education Status", "Total Store Sales"),
    ("customer_demographics", "Credit Rating", "Total Store Sales"),
    ("ship_mode", "Ship Mode Type", "Web Sales"),
    ("ship_mode", "Carrier", "Web Sales"),
    ("sold_date_week_hierarchy", "Sold Calendar Week", "Total Store Sales"),
    # qwf20 filter levels (domain coverage expansion — PRD-mqo-catalog-level-domain-metadata)
    ("product_dimension", "Item Color", "Total Store Sales"),
    ("product_dimension", "Product Manager ID", "Total Store Sales"),
    ("store_dimension", "Store Name", "Total Store Sales"),
    ("store_dimension", "Store City", "Total Store Sales"),
    ("sold_time_dimension", "Sold Hour", "Total Store Sales"),
    ("sold_time_dimension", "Sold Meal Time", "Total Store Sales"),
    ("sold_date_dimensions", "Sold Day of Week", "Total Store Sales"),
    ("customer_demographics", "Buy Potential", "Total Store Sales"),
    ("customer_demographics", "Household Dependents", "Total Store Sales"),
    ("customer_demographics", "Vehicle Count", "Total Store Sales"),
    ("customer_address", "Customer GMT Offset", "Total Store Sales"),
    ("customer_address", "Customer State", "Total Store Sales"),
    ("store_dimension", "Store GMT Offset", "Total Store Sales"),
    ("store_dimension", "Store Floor Space", "Total Store Sales"),
    ("promotion_dimension", "Promotion Channel", "Total Store Sales"),
    ("promotion_dimension", "In Promotion", "Total Store Sales"),
    ("item_dimension", "Item Class Name", "Total Store Sales"),
]

INT_RE = re.compile(r"-?\d+")
DATE_RE = re.compile(r"\d{4}-\d{2}-\d{2}")


def value_type(samples):
    if all(DATE_RE.fullmatch(str(s)) for s in samples):
        return "date"
    if all(INT_RE.fullmatch(str(s)) for s in samples):
        return "integer"
    return "string"


def main():
    p = subprocess.Popen(
        LAUNCH, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, bufsize=1, env=os.environ,
    )

    def send(o):
        p.stdin.write(json.dumps(o) + "\n")
        p.stdin.flush()

    def recv(t=120):
        while True:
            rl, _, _ = select.select([p.stdout], [], [], t)
            if not rl:
                return None
            ln = p.stdout.readline()
            if not ln:
                return None
            try:
                o = json.loads(ln)
            except Exception:
                continue
            if o.get("id"):
                return o

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
          "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                     "clientInfo": {"name": "capture", "version": "0"}}})
    recv()

    out = {}  # hierarchy -> [ {level, value_type, domain?, expected_key_shape?} ]
    rid = 1
    for hier, level, meas in TARGETS:
        rid += 1
        mqo = {"model": "tpcds_benchmark_model",
               "measures": [{"unique_name": meas}],
               "dimensions": [{"hierarchy": hier, "level": level}],
               "filters": [], "time_intelligence": [], "non_empty": True,
               "limit": CAP + 1}
        send({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
              "params": {"name": "query_multidimensional",
                         "arguments": {"mqo": mqo}}})
        resp = recv()
        members = []
        try:
            res = resp["result"]
            txt = res["content"][0]["text"] if "content" in res else json.dumps(res)
            o = json.loads(txt)
            sc = o.get("structuredContent") or o
            rows = sc.get("rows") or sc.get("page") or []
            for r in rows:
                if not isinstance(r, dict):
                    continue
                # XMLA-mangled column keys: a MEASURE is `[Name]` →
                # `_x005b_..._x005d_`; a dimension level is table-qualified
                # `atscale_catalogs[Name]` → `atscale_catalogs_x005b_...`. So the
                # dimension column is the key NOT starting with the `_x005b_`
                # measure marker. This works for both string and numeric levels.
                dim_key = next((k for k in r if not k.startswith("_x005b_")), None)
                if dim_key is None:
                    continue
                val = r[dim_key]
                if val is None or val == "":
                    continue  # null member — skip
                # Integer-valued floats (e.g. a 5159.0 week sequence) → "5159".
                if isinstance(val, float) and val.is_integer():
                    val = int(val)
                members.append(str(val))
        except Exception as e:
            print(f"# skip {hier}.{level}: {str(e)[:80]}", file=sys.stderr)
            continue
        members = sorted(set(members))
        if not members:
            print(f"# skip {hier}.{level}: no members", file=sys.stderr)
            continue
        vt = value_type(members[:25])
        entry = {"level": level, "value_type": vt}
        if len(members) <= CAP:
            entry["domain"] = members
        else:
            entry["expected_key_shape"] = (
                f"{vt} member key; {len(members)}+ distinct values "
                f"(high-cardinality, domain not enumerated)"
            )
        out.setdefault(hier, []).append(entry)
        print(f"# {hier}.{level}: {vt}, "
              f"{len(members) if len(members) <= CAP else str(len(members)) + '+ (descriptor)'}",
              file=sys.stderr)

    try:
        p.stdin.close(); p.terminate()
    except Exception:
        pass
    json.dump(out, sys.stdout, indent=2)


if __name__ == "__main__":
    main()
