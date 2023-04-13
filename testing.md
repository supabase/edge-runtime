## Testing the Edge Runtime

In order to run the test in the repository, make sure the docker daemon is running and create a docker image:

```bash
docker build -t edge-runtime:test .
```

Install tests dependencies:

```bash
cd test
npm install
```

Run tests:

```bash
npm run test
```