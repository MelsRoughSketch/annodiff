"""Exercise the publish workflow's validation without contacting registries."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import textwrap
import urllib.error
from unittest.mock import patch

workflow = Path(__file__).resolve().parents[1] / '.github/workflows/publish.yml'
source = workflow.read_text().split("          python3 - <<'PY'\n", 1)[1].split('          PY\n', 1)[0]
code = compile(textwrap.dedent(source), str(workflow), 'exec')


def check(*, draft=False, prerelease=False, tag='v0.2.0', release_tag='v0.2.0',
          version='0.2.0', name='annodiff', status=404, error=None, publish=True):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "{version}"\n')
        (root / 'release.json').write_text(json.dumps(dict(
            tagName=release_tag, isDraft=draft, isPrerelease=prerelease)))
        output = root / 'output'
        with contextlib.chdir(root), patch.dict(os.environ, {
            'RUNNER_TEMP': tmp, 'RELEASE_TAG': tag, 'GITHUB_OUTPUT': str(output)
        }), patch('subprocess.check_output', return_value='abc123\n') as git, \
             patch('urllib.request.urlopen') as request, contextlib.redirect_stdout(io.StringIO()):
            if status == 200:
                request.return_value = io.BytesIO()
            else:
                request.side_effect = urllib.error.HTTPError('https://example.invalid', status, '', None, None)
            try:
                exec(code, {})
            except (SystemExit, urllib.error.HTTPError) as exc:
                assert error is not None and error in str(exc), str(exc)
                assert not output.exists(), 'Failed validation must not emit publish outputs'
                if isinstance(exc, SystemExit):
                    request.assert_not_called()
            else:
                assert error is None, 'Expected validation failure'
                assert output.read_text() == f'sha=abc123\npublish={str(publish).lower()}\n'
                git.assert_called_once_with(['git', 'rev-parse', 'HEAD'], text=True)
                assert request.call_args.args[0].full_url == 'https://crates.io/api/v1/crates/annodiff/0.2.0'


check()
check(status=200, publish=False)
check(draft=True, error='Only published stable')
check(prerelease=True, error='Only published stable')
check(tag='v0.3.0', error='Release tag must match')
check(release_tag='v0.3.0', error='Release tag must match')
check(version='0.3.0', error='Release tag must match')
check(name='other', error='Release tag must match')
check(status=403, error='403')
check(status=500, error='500')
print('PASS: release validation, immutable commit output, duplicate publication, and API failures')
