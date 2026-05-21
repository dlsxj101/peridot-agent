import * as path from 'path';

/** Environment inherited by Peridot child processes.
 *
 * Cursor's Windows extension host can provide USERPROFILE without HOME.
 * Peridot accepts USERPROFILE natively, but setting HOME as well keeps older
 * bundled binaries and any subprocesses they spawn on the happy path.
 */
export function peridotChildEnv(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env };
  if (!env.HOME || env.HOME.trim().length === 0) {
    const home = windowsHome(env);
    if (home) env.HOME = home;
  }
  return env;
}

function windowsHome(env: NodeJS.ProcessEnv): string | undefined {
  if (env.USERPROFILE && env.USERPROFILE.trim().length > 0) {
    return env.USERPROFILE;
  }
  if (env.HOMEDRIVE && env.HOMEPATH) {
    return path.join(env.HOMEDRIVE, env.HOMEPATH);
  }
  return undefined;
}
