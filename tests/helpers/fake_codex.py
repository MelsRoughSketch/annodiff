import json, os, pathlib, re, sys, time
if sys.argv[1:] == ["app-server"]:
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request["method"]
        with open(os.environ["FAKE_RPC_LOG"], "a") as log:
            log.write(json.dumps(dict(request, pid=os.getpid())) + "\n")
        behavior_file = pathlib.Path(os.environ["FAKE_RPC_BEHAVIOR"])
        behavior = behavior_file.read_text() if behavior_file.exists() else ""
        if behavior == "hang":
            time.sleep(60)
        elif behavior == "error":
            print(json.dumps({"id": request["id"], "error": {"message": "injected failure"}}), flush=True)
            continue
        elif behavior == "exit":
            sys.exit(0)
        result = {}
        if method == "config/read":
            assert request["params"]["cwd"] == os.environ["FAKE_CWD"], request
            result = {"config": {"model_provider": "test-provider", "features": {"worktrees": True}}}
        elif method == "thread/list":
            params = request["params"]
            assert params["sourceKinds"] == ["cli", "vscode"], params
            assert params["modelProviders"] == ["test-provider"], params
            assert isinstance(params["archived"], bool), params
            assert os.getcwd() == os.environ["FAKE_CWD"], os.getcwd()
            assert params["cwd"] in (None, [os.getcwd()]), params
            db_only = params["useStateDbOnly"]
            mode = os.environ["FAKE_DB_MODE"]
            if mode == "ready":
                assert db_only, "must not scan rollouts when DB has results"
            if db_only and mode == "error":
                print(json.dumps({"id": request["id"], "error": {"message": "DB unavailable"}}), flush=True)
                continue
            def row(id, source="cli", provider="test-provider", archived=False, cwd=None, updated=20, created=10):
                return dict(id=id, name=None, preview="ARCHIVED_FIXTURE" if archived else "Unnamed session preview " + id,
                            cwd=cwd or os.getcwd(), updatedAt=updated, createdAt=created,
                            source=source, modelProvider=provider, archived=archived)
            rows = [row("unnamed"), row("second", source="vscode", updated=10, created=30),
                    row("elsewhere", cwd="/other-project", updated=40, created=40),
                    row("archived", archived=True), row("other-provider", provider="other")]
            rows += [row("root-" + str(i), cwd=str(pathlib.Path(os.getcwd()).parent)) for i in range(30)]
            rows += [row("exec-" + str(i), source="exec") for i in range(30)]
            rows = [r for r in rows if r["source"] in params["sourceKinds"]
                    and r["modelProvider"] in params["modelProviders"] and r["archived"] == params["archived"]]
            if params["cwd"] is not None:
                rows = [r for r in rows if r["cwd"] in params["cwd"]]
            if db_only and mode == "empty":
                rows = []
            rows.sort(key=lambda r: r["createdAt" if params["sortKey"] == "created_at" else "updatedAt"], reverse=True)
            offset = int(params.get("cursor") or 0)
            result = {"data": rows[offset:offset+2], "nextCursor": str(offset+2) if offset+2 < len(rows) else None}
        elif method == "thread/unarchive":
            assert request["params"]["threadId"] == "archived"
            pathlib.Path(os.environ["FAKE_PAYLOAD"]).with_suffix(".unarchive").write_text("restored")
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
else:
    assert os.getcwd() == os.environ["FAKE_CWD"], os.getcwd()
    prompt = sys.argv[-1]
    match = re.search(r'Read the review in (".*?") and address', prompt)
    payload = pathlib.Path(os.environ["FAKE_PAYLOAD"])
    payload.write_text(pathlib.Path(json.loads(match[1])).read_text() if match else prompt)
    payload.with_suffix(".mode").write_text("file" if match else "direct")
    print("fake-codex-finished", flush=True)
    sys.exit(int(os.environ["FAKE_FAIL"]))
