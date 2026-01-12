#!/usr/bin/env bash
exec cargo llvm-cov --html --features shell-integration-tests "$@"
