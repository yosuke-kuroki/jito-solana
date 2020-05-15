# Validation-client Economics

**Subject to change.**

Validator-clients are eligible to receive protocol-based \(i.e. inflation-based\) rewards issued via stake-based annual interest rates \(calculated per epoch\) by providing compute \(CPU+GPU\) resources to validate and vote on a given PoH state. These protocol-based rewards are determined through an algorithmic disinflationary schedule as a function of total amount of circulating tokens. The network is expected to launch with an annual inflation rate around 15%, set to decrease by 15% per year until a long-term stable rate of 1-2% is reached. These issuances are to be split and distributed to participating validators, with around 90% of the issued tokens allocated for validator rewards. Because the network will be distributing a fixed amount of inflation rewards across the stake-weighted validator set, any individual validator's interest rate will be a function of the amount of staked SOL in relation to the circulating SOL.

Additionally, validator clients may earn revenue through fees via state-validation transactions. For clarity, we separately describe the design and motivation of these revenue distributions for validation-clients below: state-validation protocol-based rewards and state-validation transaction fees and rent.

