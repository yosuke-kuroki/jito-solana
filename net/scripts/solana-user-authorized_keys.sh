#
# Contains the public keys for users that should automatically be granted access
# to ALL testnets and datacenter nodes.
#
# To add an entry into this list:
# 1. Run: ssh-keygen -t ecdsa -N '' -f ~/.ssh/id-solana-testnet
# 2. Add an entry to SOLANA_USERS with your username
# 3. Add an entry to SOLANA_PUBKEYS with the contents of ~/.ssh/id-solana-testnet.pub
#
# If you need multiple keys with your username, repeatedly add your username to SOLANA_USERS, once per key
#

SOLANA_USERS=()
SOLANA_PUBKEYS=()

SOLANA_USERS+=('mvines')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFBNwLw0i+rI312gWshojFlNw9NV7WfaKeeUsYADqOvM2o4yrO2pPw+sgW8W+/rPpVyH7zU9WVRgTME8NgFV1Vc=')

SOLANA_USERS+=('sathish')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBGqZAwAZeBl0buOMz4FpUYrtpwk1L5aGKlbd7lI8dpbSx5WVRPWCVKhWzsGMtDUIfmozdzJouk1LPyihghTDgsE=')

SOLANA_USERS+=('carl')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBOk4jgcX/VWSk3j//wXeIynSQjsOt+AjYXM/XZUMa7R1Q8lfIJGK/qHLBP86CMXdpyEKJ5i37QLYOL+0VuRy0CI=')

SOLANA_USERS+=('jack')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEB6YLY4oCfm0e1qPswbzryw0hQEMiVDcUxOwT4bdBbui/ysKGQlVY8bO6vET1Te8EYHz5W4RuPfETbcHmw6dr4=')

SOLANA_USERS+=('trent')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEZC/APgZTM1Y/EfNnCHr+BQN+SN4KWfpyGkwMg+nXdC trent@fry')
SOLANA_USERS+=('trent')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDgdbzGLiv9vGo3yaJGzxO3Q2/w5TS4Km2sFGQFWGFIJ trent@farnsworth')
SOLANA_USERS+=('trent')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHD7QmrbCqEFYGmYlHNsfbAqmJ6FRvJUKZap1TWMc7Sz trent@Trents-MacBook-Pro.local')
SOLANA_USERS+=('trent')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN2NCuglephBlrWSpaLkGFdrAz1aA3vYHjBVJamWBCZ3 trent@trent-build')

SOLANA_USERS+=('tristan')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ9VNoG7BLPNbyr4YLf3M2LfQycvFclvi/giXvTpLp0b tristan@TristanSolanaMacBook.local')

SOLANA_USERS+=('dan')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBKMl07qHaMCmnvRKBCmahbBAR6GTWkR5BVe8jdzDJ7xzjXLZlf1aqfaOjt5Cu2VxvW7lUtpJQGLJJiMnWuD4Zmc= dan@Dans-MBP.local')
SOLANA_USERS+=('dan')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBLG+2CSMwuSjX1l4ke7ScGOgmE2/ZICvJUg6re5w5znPy1gZ3YenypoBkoj3mWmavJ09OrUAELzYj3YQU9tSVh4= dan@cabbage-patch')

SOLANA_USERS+=('greg')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG3eu2c7DZS+FE3MZmtU+nv1nn9RqW0lno0gyKpGtxT7 greg@solana.com')

SOLANA_USERS+=('tyera')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBDSWMrqTMsML19cDKmxhfwkDfMWwpcVSYJ49cYkZYpZfTvFjV/Wdbpklo0+fp98i5AzfNYnvl0oxVpFg8A8dpYk=')

#valverde/sagan
SOLANA_USERS+=('sakridge')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIxN1jPgVdSqNmGAjFwA1ypcnME8uM/9NjfaUZBpNdMh sakridge@valverde')
#fermi
SOLANA_USERS+=('sakridge')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILADsMxP8ZtWxpuXjqjMcYpw6d9+4rgdYrmrMEvrLtmd sakridge@fermi.local')
SOLANA_USERS+=('sakridge')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF5JFfLo8rNBDV6OY08n/BWWu/AMCt6KAQ+2syeR+bvY sakridge@curie')

SOLANA_USERS+=('buildkite-agent')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHnXXGKZF1/qjhsoRp+7Dm124nIgUbGJPFoqlSkagZmGmsqqHlxgosxHhg6ucHerqonqBXtfdmA7QkZoKVzf/yg= buildkite-agent@dumoulin')
#ci-testnet-deployer
SOLANA_USERS+=('buildkite-agent')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJnESaQpgLM2s3XLW2jvqRrvkBMDd/qGDZCjPR4X/73IwiR+hSw220JaT1JlweRrEh0rodgBTCFsWYSeMbLeGu4= buildkite-agent@ci-testnet-deployer')

SOLANA_USERS+=('pankaj')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPBLR4Z2HbksF+MUFmdjf5jkWoMWB0JC9a0Bz0OHvrvp pankaj@Pankajs-MacBook-Pro.local')

SOLANA_USERS+=('jstarry')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBCdpItheyXVow+4j1D4Y8Xh+dsS9GwFLRNiEYjvnonV3FqVO4hss6gmXPk2aiOAZc6QW3IXBt/YebWFNsxBW2xU= jstarry@Justin-Solana.local')

SOLANA_USERS+=('sunny')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBNYbZAtB2Z1GOVReC/v8tkebKh/nvIyA8X6iVCffl6uxy+HNfgSs7UKLSYufCXeVdF2FGuvADPB+K6j/kgzuEow= sunny@solana.com')

SOLANA_USERS+=('sdhawan')
SOLANA_PUBKEYS+=('ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBKSbp1TUgr88QNqMy9OotOrfa10ZXcqVd/lKl4qtD+OTt2I7gWg0HL1BiYBTziiGYSdzOQSv9FkZ7f8IJNOWSmA= sdhawan@sdhawan-Blade')

SOLANA_USERS+=('ryoqun')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAsOLWUu8wbe6C5IdyB+gy1KwPCggiWv2UwhWRNOI6kV ryoqun@ubuqun')

SOLANA_USERS+=('aeyakovenko')
SOLANA_PUBKEYS+=('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEl4U9VboeYbZNOVn8SB1NM29HeI3SwqsbM22Jmw6975 aeyakovenko@valverde')
