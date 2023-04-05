REPO=$(git rev-parse --show-toplevel)
BUILDS=${BUILDS:-target/release}
# This assumes the server is already running
hyperfine \
  --command-name "start-up-server-req" "curl --location --request GET 'http://localhost:9000/bar'" \
  --runs 5 \
  --warmup 5 \
  --time-unit "millisecond" \
  --export-json "results.json";