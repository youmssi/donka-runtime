import { pathToFileURL } from 'node:url';

const candidatePattern = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)-rc\.[1-9]\d*$/;
const commitPattern = /^[a-f0-9]{40}$/;
const scriptUrl = process.argv[1] ? pathToFileURL(process.argv[1]).href : '';

export function candidatePlan({ version, ref, sha }) {
  if (!candidatePattern.test(version ?? '')) {
    throw new Error('Candidate version must use X.Y.Z-rc.N with a positive, non-zero-padded N.');
  }
  if (ref !== 'develop') throw new Error('Candidate builds must be dispatched from develop.');
  if (!commitPattern.test(sha ?? '')) throw new Error('Candidate source must be identified by a full commit SHA.');

  return {
    version,
    releaseTag: `v${version}`,
    imageTags: [version],
    assetName: `candidate-${version}.json`,
    sourceCommit: sha,
  };
}

if (import.meta.url === scriptUrl) {
  try {
    const [version, ref, sha] = process.argv.slice(2);
    const plan = candidatePlan({ version, ref, sha });
    const output = [
      `version=${plan.version}`,
      `release_tag=${plan.releaseTag}`,
      `image_tag=${plan.imageTags[0]}`,
      `asset_name=${plan.assetName}`,
      `source_commit=${plan.sourceCommit}`,
    ].join('\n') + '\n';
    if (process.env.GITHUB_OUTPUT) {
      const { appendFile } = await import('node:fs/promises');
      await appendFile(process.env.GITHUB_OUTPUT, output);
    } else {
      process.stdout.write(output);
    }
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
