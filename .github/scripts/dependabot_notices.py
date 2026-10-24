"""Commit CI-generated notices to the unchanged Dependabot head, then dispatch CI."""
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile


def api(endpoint, payload=None):
    command = ['gh', 'api', endpoint]
    if payload is not None:
        command += ['--input', '-']
    result = subprocess.check_output(command, text=True,
                                     input=json.dumps(payload) if payload is not None else None)
    return json.loads(result)


def main():
    repo = os.environ['GITHUB_REPOSITORY']
    run = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['workflow_run']
    if (run['event'] != 'pull_request' or run['path'] != '.github/workflows/ci.yml'
            or run['head_repository']['full_name'] != repo
            or run['conclusion'] not in ('success', 'failure')):
        return
    root = f'repos/{repo}'
    prs = api(f"{root}/commits/{run['head_sha']}/pulls?per_page=100")
    prs = [pr for pr in prs if pr['state'] == 'open'
           and pr['user']['login'] == 'dependabot[bot]'
           and pr['base']['ref'] == 'main' and pr['base']['repo']['full_name'] == repo
           and pr['head']['repo'] and pr['head']['repo']['full_name'] == repo
           and pr['head']['ref'].startswith('dependabot/cargo/')
           and pr['head']['ref'] == run['head_branch']
           and pr['head']['sha'] == run['head_sha']]
    if len(prs) != 1:
        print('No current, same-repository Dependabot Cargo PR; skipping.')
        return
    pr = prs[0]
    artifacts = api(f"{root}/actions/runs/{run['id']}/artifacts?name=dependabot-notices")
    if not artifacts['artifacts']:
        print('No regenerated notices; skipping.')
        return
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(['gh', 'run', 'download', str(run['id']), '--repo', repo,
                        '--name', 'dependabot-notices', '--dir', tmp], check=True)
        notice = Path(tmp) / 'THIRD_PARTY_NOTICES.md'
        if notice.is_symlink() or not notice.is_file():
            raise ValueError('Expected a regular THIRD_PARTY_NOTICES.md artifact')
        content = notice.read_bytes()
    if not content.startswith(b'# Third-party notices\n'):
        raise ValueError('Invalid third-party notices artifact')
    filename = 'THIRD_PARTY_NOTICES.md'
    current = api(f"{root}/contents/{filename}?ref={run['head_sha']}")
    if base64.b64decode(current['content']) == content:
        print('Notices already match; skipping.')
        return
    # expectedHeadOid rejects concurrent rebases or pushes without overwriting them.
    result = api('graphql', {
        'query': '''mutation($input: CreateCommitOnBranchInput!) {
          createCommitOnBranch(input: $input) { commit { oid } }
        }''',
        'variables': {'input': {
            'branch': {'repositoryNameWithOwner': repo, 'refName': pr['head']['ref']},
            'expectedHeadOid': run['head_sha'],
            'message': {'headline': 'chore: regenerate third-party notices'},
            'fileChanges': {'additions': [{
                'path': filename, 'contents': base64.b64encode(content).decode('ascii'),
            }]},
        }},
    })
    if result.get('errors'):
        raise RuntimeError(result['errors'])
    sha = result['data']['createCommitOnBranch']['commit']['oid']
    print(f"Updated PR #{pr['number']} with {sha}")
    # Explicit dispatch also works when GITHUB_TOKEN pushes do not start PR CI.
    subprocess.run(['gh', 'workflow', 'run', 'ci.yml', '--repo', repo,
                    '--ref', pr['head']['ref']], check=True)


if __name__ == '__main__':
    main()
