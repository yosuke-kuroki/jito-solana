---
title: Validator Requirements
---

## Hardware Recommendations

- CPU
  - 12 cores / 24 threads, or more
  - 2.8GHz, or faster
  - AVX2 instruction support (to use official release binaries, self-compile
otherwise)
  - Support for AVX512f and/or SHA-NI instructions is helpful
  - The AMD Threadripper Zen3 series is popular with the validator community
- RAM
  - 128GB, or more
  - Motherboard with 256GB capacity suggested
- Disk
  - PCIe Gen3 x4 NVME SSD, or better
  - Accounts: 500GB, or larger. High TBW (Total Bytes Written)
  - Ledger: 1TB or larger. High TBW suggested
  - OS: (Optional) 500GB, or larger. SATA OK
  - The OS may be installed on the ledger disk, though testing has shown better
performance with the ledger on its own disk
  - Accounts and ledger _can_ be stored on the same disk, however due to high
IOPS, this is not recommended
  - The Samsung 970 and 980 Pro series SSDs are popular with the validator community
- GPUs
  - Not strictly necessary at this time
  - Motherboard and power supply speced to add one or more high-end GPUs in the
future suggested

## Virtual machines on Cloud Platforms

While you can run a validator on a cloud computing platform, it may not
be cost-efficient over the long term.

However, it may be convenient to run non-voting api nodes on VM instances for
your own internal usage. This use case includes exchanges and services built on
Solana.

In fact, the mainnet-beta validators operated by the team are currently
(Mar. 2021) run on GCE `n2-standard-32` (32 vCPUs, 128 GB memory) instances with
2048 GB SSD for operational convenience.

For other cloud platforms, select instance types with similar specs.

Also note that egress internet traffic usage may turn out to be high,
especially for the case of running staked validators.

## Docker

Running validator for live clusters (including mainnet-beta) inside Docker is
not recommended and generally not supported. This is due to concerns of general
docker's containerzation overhead and resultant performance degradation unless
specially configured.

We use docker only for development purpose.

## Software

- We build and run on Ubuntu 20.04.
- See [Installing Solana](../cli/install-solana-cli-tools.md) for the current Solana software release.

Be sure to ensure that the machine used is not behind a residential NAT to avoid
NAT traversal issues. A cloud-hosted machine works best. **Ensure that IP ports 8000 through 10000 are not blocked for Internet inbound and outbound traffic.**
For more information on port forwarding with regards to residential networks,
see [this document](http://www.mcs.sdsmt.edu/lpyeatt/courses/314/PortForwardingSetup.pdf).

Prebuilt binaries are available for Linux x86_64 on CPUs supporting AVX2 \(Ubuntu 20.04 recommended\).
MacOS or WSL users may build from source.

## GPU Requirements

CUDA is required to make use of the GPU on your system. The provided Solana
release binaries are built on Ubuntu 20.04 with [CUDA Toolkit 10.1 update 1](https://developer.nvidia.com/cuda-toolkit-archive). If your machine is using
a different CUDA version then you will need to rebuild from source.
