#!/bin/bash -ex

[[ $(uname) = Linux ]] || exit 1
[[ $USER = root ]] || exit 1

adduser solana --gecos "" --disabled-password --quiet
adduser solana sudo
echo "solana ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers
id solana

[[ -r /solana-id_ecdsa ]] || exit 1
[[ -r /solana-id_ecdsa.pub ]] || exit 1

sudo -u solana bash -c "
  mkdir -p /home/solana/.ssh/
  cd /home/solana/.ssh/
  cp /solana-id_ecdsa.pub authorized_keys
  umask 377
  cp /solana-id_ecdsa id_ecdsa
  echo \"
    Host *
    BatchMode yes
    IdentityFile ~/.ssh/id_ecdsa
    StrictHostKeyChecking no
  \" > config
"

