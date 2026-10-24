"""Exercise the notice publisher's trust boundaries without writing to GitHub."""
import base64
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch

script = Path(__file__).resolve().parents[1] / '.github/scripts/dependabot_notices.py'
spec = importlib.util.spec_from_file_location('publisher', script)
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
repo = 'owner/annodiff'
head = {'repo': {'full_name': repo}, 'ref': 'dependabot/cargo/example-1.2.3', 'sha': 'a' * 40}
pull = {'number': 80, 'state': 'open', 'user': {'login': 'dependabot[bot]'},
        'base': {'ref': 'main', 'repo': {'full_name': repo}}, 'head': head}
workflow_run = {'id': 123, 'event': 'pull_request', 'path': '.github/workflows/ci.yml',
                'head_repository': {'full_name': repo}, 'head_branch': head['ref'],
                'head_sha': head['sha'], 'conclusion': 'failure'}
notice = b'# Third-party notices\n\nUpdated inventory.\n'


def check(*, pr=None, run=None, artifact=True, same=False, invalid=False,
          symlink=False, conflict=False, dispatch_failure=False, writes=0, error=None):
    pr = copy.deepcopy(pull if pr is None else pr)
    run = copy.deepcopy(workflow_run if run is None else run)
    mutations = []
    dispatches = []

    def api(endpoint, payload=None):
        if '/commits/' in endpoint:
            assert head['sha'] in endpoint
            return [pr]
        if '/artifacts?' in endpoint:
            assert '/runs/123/' in endpoint
            return {'artifacts': [{'name': 'dependabot-notices'}] if artifact else []}
        if '/contents/' in endpoint:
            assert endpoint.endswith(f"THIRD_PARTY_NOTICES.md?ref={head['sha']}")
            return {'content': base64.b64encode(notice if same else b'old').decode()}
        assert endpoint == 'graphql'
        commit = payload['variables']['input']
        assert commit['expectedHeadOid'] == head['sha']
        assert commit['branch'] == {'repositoryNameWithOwner': repo, 'refName': head['ref']}
        assert commit['fileChanges'] == {'additions': [{
            'path': 'THIRD_PARTY_NOTICES.md', 'contents': base64.b64encode(notice).decode(),
        }]}
        mutations.append(commit)
        if conflict:
            return {'errors': [{'message': 'Head changed'}]}
        return {'data': {'createCommitOnBranch': {'commit': {'oid': 'b' * 40}}}}

    def command(args, *, check):
        assert check
        if args[:3] == ['gh', 'run', 'download']:
            assert args[3:9] == ['123', '--repo', repo, '--name', 'dependabot-notices', '--dir']
            target = Path(args[-1]) / 'THIRD_PARTY_NOTICES.md'
            if symlink:
                target.symlink_to(script)
            else:
                target.write_bytes(b'bad' if invalid else notice)
        else:
            assert args == ['gh', 'workflow', 'run', 'ci.yml', '--repo', repo, '--ref', head['ref']]
            dispatches.append(args)
            if dispatch_failure:
                raise subprocess.CalledProcessError(1, args)

    with tempfile.TemporaryDirectory() as tmp:
        event = Path(tmp) / 'event.json'
        event.write_text(json.dumps({'workflow_run': run}))
        with patch.dict(os.environ, GITHUB_REPOSITORY=repo, GITHUB_EVENT_PATH=str(event)), \
             patch.object(publisher, 'api', side_effect=api), \
             patch.object(publisher.subprocess, 'run', side_effect=command), \
             contextlib.redirect_stdout(io.StringIO()):
            try:
                publisher.main()
            except (ValueError, RuntimeError, subprocess.CalledProcessError) as exc:
                assert error is not None and error in str(exc), str(exc)
            else:
                assert error is None, 'Expected failure'
    assert len(mutations) == writes
    assert len(dispatches) == (writes if not conflict else 0)


check(writes=1)
check(run={**workflow_run, 'conclusion': 'success'}, writes=1)
for field, value in [('event', 'push'), ('path', '.github/workflows/other.yml'),
                     ('head_repository', {'full_name': 'fork/annodiff'}),
                     ('conclusion', 'cancelled')]:
    check(run={**workflow_run, field: value})
for field, value in [('state', 'closed'), ('user', {'login': 'someone'}),
                     ('base', {'ref': 'other', 'repo': {'full_name': repo}}),
                     ('base', {'ref': 'main', 'repo': {'full_name': 'fork/annodiff'}})]:
    check(pr={**pull, field: value})
for field, value in [('repo', None), ('repo', {'full_name': 'fork/annodiff'}),
                     ('ref', 'main'), ('ref', 'dependabot/cargo/another-branch'),
                     ('sha', 'c' * 40)]:
    check(pr={**pull, 'head': {**head, field: value}})
check(artifact=False)
check(same=True)
check(invalid=True, error='Invalid third-party')
check(symlink=True, error='Expected a regular')
check(conflict=True, writes=1, error='Head changed')
check(dispatch_failure=True, writes=1, error='non-zero exit status')

# Exercise the actual gh transport, including structured GraphQL variables.
with patch.object(publisher.subprocess, 'check_output', return_value='{"ok": true}') as gh:
    assert publisher.api('graphql', {'variables': {'value': 'a\nb'}}) == {'ok': True}
    assert json.loads(gh.call_args.kwargs['input']) == {'variables': {'value': 'a\nb'}}
    assert gh.call_args.args[0] == ['gh', 'api', 'graphql', '--input', '-']
print('PASS: Dependabot eligibility, artifact validation, concurrent updates, and CI dispatch')
