// This file is meant to only have `userRuntimeCleanUp`
// The code should address any user specific runtime behavior
// As well as deletions

const deleteDenoApis = (apis) => {
    apis.forEach((key) => {
       delete Deno[key];
    });
}

function loadUserRuntime() {
    deleteDenoApis(['chdir', 'chmod', 'chmodSync', 'chown', 'chownSync', 'copyFile', 'copyFileSync', 'create', 'createSync', 'cwd', 'fdatasync',
        'fdatasyncSync', 'File', 'flock', 'flockSync', 'FsFile', 'fstat', 'fstatSync', 'fsync', 'fsyncSync', 'ftruncate', 'ftruncateSync',
        'funlock', 'funlockSync', 'futime', 'futimeSync', 'link', 'linkSync', 'lstat', 'lstatSync', 'makeTempDir', 'makeTempDirSync',
        'makeTempFile', 'makeTempFileSync', 'mkdir', 'mkdirSync', 'open', 'openSync', 'readDir', 'readDirSync', 'readFile',
        'readFileSync', 'readLink', 'readLinkSync', 'readTextFile', 'readTextFileSync', 'realPath', 'realPathSync', 'remove',
        'removeSync', 'rename', 'renameSync', 'seek', 'seekSync', 'stat', 'statSync', 'symlink', 'symlinkSync', 'truncate',
        'truncateSync', 'umask', 'utime', 'utimeSync', 'writeFile', 'writeFileSync', 'writeTextFile', 'writeTextFileSync']);
    delete globalThis.EdgeRuntime;
}

export { loadUserRuntime };