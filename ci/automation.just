# Offline automation and reporting recipes; no native build inputs.

inference-contracts:
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/inference_contracts.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/inference_markdown.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/inference_nightly.py

inference-publication-test:
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/inference_publication.py

automation-contracts: inference-contracts
    @"{{ automation_python }}" scripts/tests/ci_cuda_license.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/automation_contracts.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/upgrade_contracts.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/graph_contracts.py
    @PYTHONPATH=scripts "{{ automation_python }}" scripts/tests/release_contracts.py

[positional-arguments]
inference-render-report format input *options:
    @"{{ automation_python }}" scripts/inference-render-report.py "$@"

ci-automation: automation-contracts inference-publication-test
    @bash scripts/ci-automation.sh

[positional-arguments]
ci-automation-setup directory:
    @bash scripts/ci-automation-setup.sh "$@"
