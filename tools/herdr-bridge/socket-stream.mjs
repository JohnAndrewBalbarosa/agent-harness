import net from 'node:net';

const socketPath = process.argv[2];
if (!socketPath) process.exit(2);

const address = process.platform === 'win32' ? `\\\\.\\pipe\\${socketPath}` : socketPath;
const connection = net.createConnection(address);
connection.on('connect', () => process.stdin.pipe(connection));
connection.pipe(process.stdout);
connection.on('error', (error) => {
  process.stderr.write(`${error.code || error.name}\n`);
  process.exitCode = 1;
});
connection.on('close', () => {
  process.stdin.unpipe(connection);
  process.stdin.pause();
});
