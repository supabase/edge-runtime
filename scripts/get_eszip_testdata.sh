#!/bin/bash

PWD=$(pwd)
SCRIPT=$(readlink -f "$0")
SCRIPTPATH=$(dirname "$SCRIPT")
ROOTPATH=$(realpath $SCRIPTPATH/..)

usage() {
  echo "Usage: $0 <profile> <s3 bucket name> <out directory>"
}

if [[ $# -lt 3 ]]; then
  usage
  exit 1
fi

if ! command -v aws-vault &> /dev/null; then
  printf "aws-vault is not installed.\nSee for install and configure: https://github.com/99designs/aws-vault\n"
  exit 1
fi
if ! command -v node &> /dev/null; then
  echo "NodeJS is not installed."
  exit 1
fi

PROFILE_FOUND=0
PROFILES=$(aws-vault list | awk 'NR > 2 {print $1}')
for profile in $PROFILES; do
  if [ "$1" == "$profile" ]; then
    PROFILE_FOUND=1
    break
  fi
done
if [ $PROFILE_FOUND -eq 0 ]; then
  echo "Profile '$1' does not exist in aws-vault."
  exit 1
fi

PROFILE=$1
BUCKET_NAME=$2
TEMP_DIR=$(mktemp -d)
TESTDATA_DIR=$3

aws-vault exec $1 -- aws s3 cp s3://$BUCKET_NAME $TEMP_DIR --recursive
TOTAL=$(ls -1 $TEMP_DIR | wc -l)
ls -1 $TEMP_DIR | xargs -I {} $ROOTPATH/scripts/esbr.cjs decompress $TEMP_DIR/{}
VALID=$(find $TEMP_DIR -type f -name "*.out" | wc -l)
find $TEMP_DIR -type f ! -name "*.out" -delete
mkdir -p $TESTDATA_DIR
mv $TEMP_DIR/* $TESTDATA_DIR

echo "Done! (Total: $TOTAL Valid: $VALID)"
exit 0
