# Constellation Instance

A ready-to-use hosting implementation of the
[Agent Instance boundary](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/agent-instance.md).
It keeps an installed agent's identity, configuration, authorized context and
controls available across experiences. This implementation hosts a selected ACP
engine for reasoning and tool execution; other agents can use different internal
architectures while keeping the same outward promises.

The [Constellation vision](https://github.com/microsoft/amplifier-constellation/blob/main/docs/VISION.md)
explains the larger ambition. The [shared agreements](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md)
own cross-component promises. Instance can be used
without a fleet service or the other Constellation components.

## Start with the role you need

| You want to… | Start here |
| --- | --- |
| Understand the value this service should provide | [Instance vision](docs/VISION.md) |
| Design or change an interface | [Local contract map](contracts/README.md) |
| Install an independent agent | [Consumer packages and native startup](docs/native-platforms.md) |
| Connect a client or experience | [ACP boundary](docs/acp-boundary.md) and [technical interface map](contracts/README.md#technical-interfaces) |
| Operate an installation programmatically | [Management API](docs/api.md) |
| Change code and verify it | [Working here](AGENTS.md) and [validation](docs/validation.md) |

The bundled browser controls are a development and administration client of the
same service interfaces. They demonstrate accessible controls; their layout does
not define the UX of future products.

## Install

Download the matching **consumer bundle** from a published release and keep its
files together. It contains the native executable, a private Python interpreter,
and fixed setup support; no WSL, compiler, Git, Node, or system Python is needed.
An optional fleet client can invoke the same setup interface.

```sh
./agent-instance setup --home "/path/to/my-agent" --name "My agent"
```

On Windows run `agent-instance.exe` with the same arguments. The native engine
comes only from the checked release lock. An unavailable platform is reported;
setup never substitutes another engine or builds source. Provider access remains
an explicit account or credential connection after installation.

[Native platforms](docs/native-platforms.md) describes package support, OS account
requirements, startup and the fixed host recipe. Linux's custom PAM policy needs
an explicit administrator installation; the consumer bundle does not silently
write system authentication policy. [Development installation](docs/install.md)
and the `install*.sh` / PowerShell scripts remain optional source-build paths.
Every running installation keeps its own executable, interpreter and private state
independently of the download folder. Optional Linux containment has a separate
[per-installation recipe](ops/container/README.md).

## Use the interfaces

**ACP conducts agent interactions.** A standard stdio attach command connects a
client to the installed individual:

```text
command: /path/to/instance/bin/agent-instance
args: ["acp", "--home", "/path/to/instance"]
```

The [ACP boundary](docs/acp-boundary.md) defines relay behavior, backend capability
ownership, context insertion and the explicitly custom `agent-instance.wss/1`
remote hop. Select supported behavior from the actual backend and negotiated
interfaces, rather than from the Instance product name.

**Management operates the installation.** The CLI and bundled UI use `/api/v1`.
The [management contract](docs/api.md) defines authentication, discovery, schemas
and operations. [Reference-client controls](docs/ui-contract.md) define view
state separately from service state. A new experience can compose these
interfaces without reproducing the bundled administration screens.

The [hosting agreement](contracts/hosting.md) identifies Instance's local promises;
the [technical interface map](contracts/README.md#technical-interfaces) locates
source bindings, responsibilities, memory, lifecycle and other optional APIs.

## Local commands

```bash
agent-instance init --home ./my-agent --name "My agent"
agent-instance backend install --home ./my-agent
agent-instance serve --home ./my-agent
agent-instance status --home ./my-agent --json
agent-instance config get --home ./my-agent
agent-instance endpoints --home ./my-agent
agent-instance background status --home ./my-agent
agent-instance api GET views --home ./my-agent
```

`init` refuses to overwrite an existing installation. Use private secret files or
references for credentials. Follow the [management API](docs/api.md) and platform
setup instructions for authentication, revision checks and service changes.

## Develop and verify

Read the [component guidance](AGENTS.md) and the relevant interface before
changing it. The shared [working agreement](https://github.com/microsoft/amplifier-constellation/blob/main/docs/working-together.md)
applies Converge to work across component owners.

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

[Validation](docs/validation.md) selects installation, ACP, browser and platform
checks for the boundary being changed. Combined-path checks establish how a
consumer uses the service; a passing fixture alone does not establish model
quality or a live installation's compatibility.

## Contributing

> [!NOTE]
> This project is not currently accepting external contributions, but we're actively working toward opening this up. We value community input and look forward to collaborating in the future. For now, feel free to fork and experiment!

Most contributions require you to agree to a
Contributor License Agreement (CLA) declaring that you have the right to, and actually do, grant us
the rights to use your contribution. For details, visit [Contributor License Agreements](https://cla.opensource.microsoft.com).

When you submit a pull request, a CLA bot will automatically determine whether you need to provide
a CLA and decorate the PR appropriately (e.g., status check, comment). Simply follow the instructions
provided by the bot. You will only need to do this once across all repos using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or
contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
