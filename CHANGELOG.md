# Changelog

All notable changes to Agent Hub are recorded in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0](https://github.com/abn/agent-hub/compare/agent-hub-v1.1.0...agent-hub-v1.2.0) (2026-10-08)


### Features

* hold an artifact version live while an agent writes it ([4fef21a](https://github.com/abn/agent-hub/commit/4fef21a45b6653948b8df06ba0b8acba7362c8f8))
* **inbox:** let an agent put a deadline on a question or an approval ([efc0156](https://github.com/abn/agent-hub/commit/efc0156db4ab2a744e809345cfde2e2d354304a8))
* **inbox:** quick answers on a question ([ccd1bb3](https://github.com/abn/agent-hub/commit/ccd1bb39d6674517c2eb6a4a1e1df48a6dd7bd2c))
* nudge an operator-configured target when something waits ([2b0ef64](https://github.com/abn/agent-hub/commit/2b0ef641d132ad568130b8c8be9dbeec6570eafe))
* **ops:** take an online backup from the serving hub ([f3d3291](https://github.com/abn/agent-hub/commit/f3d3291363651ba59634e081de0ba904e0fc55f6))
* **web:** show which agent is writing an artifact live ([ababa69](https://github.com/abn/agent-hub/commit/ababa69ab8e31e99a7567ad1a37b4589e0ef0f9d))


### Bug Fixes

* **web:** hold the comments bar at the bottom ([cf3d251](https://github.com/abn/agent-hub/commit/cf3d25186062878d84b741fac4d28aa72620627e))
* **web:** redraw the composer send control ([474f36f](https://github.com/abn/agent-hub/commit/474f36fac6d37cfcaa10ff29792de60975604975))

## [1.1.0](https://github.com/abn/agent-hub/compare/agent-hub-v1.0.0...agent-hub-v1.1.0) (2026-10-06)


### ⚠ BREAKING CHANGES

* **sessions:** another agent's work, pass `from` and the hub adopts or forks it.

### Features

* **api:** name the project on storage rows, home events and search hits ([f1f2e91](https://github.com/abn/agent-hub/commit/f1f2e91e6aae1d2d9e8ea6e0c4bdc0a91fa0c5c0))
* **artifacts:** a comment control a touch device can reach ([581de02](https://github.com/abn/agent-hub/commit/581de02e953d0592eca44d914d61f05aebb78b3d))
* **artifacts:** add revocable capability share links and version pinning ([be43abc](https://github.com/abn/agent-hub/commit/be43abca3c46941fcd331a3aa9f19591ef009cff))
* **artifacts:** add share sheet and remove project password policy ([869974f](https://github.com/abn/agent-hub/commit/869974f3978855db726b427407380e2a0dae030c))
* **artifacts:** carry thread counts on listings and reads ([3635ccd](https://github.com/abn/agent-hub/commit/3635ccd84f9e9fbb5bb1e513286e2a70f0a0c605))
* **artifacts:** compact viewer title bar, glyph consolidation, and viewer chrome ([aa72978](https://github.com/abn/agent-hub/commit/aa729789734cb4da9a9f14400707fcfd994a25b9))
* **artifacts:** document comments, anchors, and margin cards ([9bf6d41](https://github.com/abn/agent-hub/commit/9bf6d41aaf415e83aea790c8ffe1ccd5374ae504))
* **artifacts:** hold a write to its project's password policy ([7971e31](https://github.com/abn/agent-hub/commit/7971e31b1aa821cba43325cc58772c1d141de81a))
* **artifacts:** redraw viewer chrome, version sheet, and grouped list ([def4b7f](https://github.com/abn/agent-hub/commit/def4b7f0826ce7985ff63871c2d44833c83f8e60))
* **brain:** accept a write naming your own session ([79e31ab](https://github.com/abn/agent-hub/commit/79e31aba9823a7fdce83c3e765b61b5674cfca80))
* **brain:** copy a brain through the engine ([6128500](https://github.com/abn/agent-hub/commit/6128500a61521638be9055e672b0d56a7c50e970))
* **brain:** give every project a knowledge base ([bd08114](https://github.com/abn/agent-hub/commit/bd08114907b48b4ffd51b265a3c75ca10bea7004))
* **brain:** promote a session entry into the project store ([f46ebd6](https://github.com/abn/agent-hub/commit/f46ebd6a2064738e540a7473ad1fb6dfa1cf5e88))
* **brain:** read another session's brain ([6c8ddb5](https://github.com/abn/agent-hub/commit/6c8ddb5f413a569b6d82b45fc13eb812ad52ebdd))
* **brain:** record and read the write log ([444e094](https://github.com/abn/agent-hub/commit/444e094dd1bb674f57777d7c035f9202f33eec8e))
* **brain:** report what each listed entry holds ([96f2fe4](https://github.com/abn/agent-hub/commit/96f2fe418c0c79f56fdf6f8ffabb83ef3695cf32))
* **build:** compile the embedded tailnet into the default build ([ed744df](https://github.com/abn/agent-hub/commit/ed744df12de4083e1c9f0c6da6a9d610de8b3b55))
* **build:** report the hub version and build commit ([478482a](https://github.com/abn/agent-hub/commit/478482affe13e4b286cbbfefa4f2e165ccd9682b))
* **cli:** call one hub tool from a shell ([c7f4638](https://github.com/abn/agent-hub/commit/c7f46386a606d6278b55bacc41eea7c7443290e5))
* **client:** proxy stdio MCP to a running hub ([24820c2](https://github.com/abn/agent-hub/commit/24820c2b4aac56c93fdf86a28c53a572f01429bf))
* **cli:** finish enrolment config and announce startup ([79df0cd](https://github.com/abn/agent-hub/commit/79df0cdc1c0aaad5b878c7a8ae1d9f8e8eb8892c))
* **cli:** print the tool listing indented, one line on request ([482b03d](https://github.com/abn/agent-hub/commit/482b03d3d5ef7a7d88fa03c1d3de9a2b7d30e3b9))
* **cli:** read and write project knowledge with kb ([fa35808](https://github.com/abn/agent-hub/commit/fa35808bc88f662d424cc78cb5ce6a131e379ee6))
* **config:** layered TOML configuration and inspect command ([d97cf7f](https://github.com/abn/agent-hub/commit/d97cf7f6d4cc1bc88fd680984ffedec2ea88deb0))
* **config:** read hub client settings from a file ([3ce18a7](https://github.com/abn/agent-hub/commit/3ce18a7dc7c86e85d6d2fb7ab6f338b2059e1776))
* **feed:** link artifact events to the artifact viewer ([c0da590](https://github.com/abn/agent-hub/commit/c0da590a184906907125bc193a335499fc449809))
* **feed:** redraw feed chips and row grammar per round 3 design ([5105053](https://github.com/abn/agent-hub/commit/5105053e44f80c1ad86f5faa71ffb8f8e296d960))
* **feed:** remember each agent's read cursor on the server ([11b80e6](https://github.com/abn/agent-hub/commit/11b80e61364428846a62521ab99bbbf1d23cd356))
* **feed:** remember how far each project feed was read ([62e619c](https://github.com/abn/agent-hub/commit/62e619c26460e069dadc4751b510bce61a25af1a))
* **home:** carry the node and the items that wait ([6e9cf91](https://github.com/abn/agent-hub/commit/6e9cf9116a279896704a450522729c449e9d18b1))
* **home:** list the waiting queue itself and name the node ([1039ef8](https://github.com/abn/agent-hub/commit/1039ef8f367df8f8d92ab1d4906650fbaa642197))
* **http:** a session detail with its size, events and last line ([d5bcef5](https://github.com/abn/agent-hub/commit/d5bcef5f0cd1c08adce0ecbdd3c4928ce82c08ed))
* **http:** brain entries carry what they are and what they hold ([0dccb1a](https://github.com/abn/agent-hub/commit/0dccb1a1e9d696ec0ecb821a60c28b5a735cdcb4))
* **http:** knowledge base REST endpoints ([5069f6d](https://github.com/abn/agent-hub/commit/5069f6dd074889eeb6cb8ec75dbe5e04e0d44638))
* **http:** one home response for every number the screen shows ([f55e371](https://github.com/abn/agent-hub/commit/f55e37109e94907a16e84ede53dac9b5bbe10423))
* **http:** read one session brain entry over REST ([e5e0c36](https://github.com/abn/agent-hub/commit/e5e0c360b51178d99d949116cff0c8495f7f99a6))
* **http:** show session ownership and lineage ([14fe42f](https://github.com/abn/agent-hub/commit/14fe42f53b45cc6febe0e52c0b2661139e3955e4))
* **identity:** remove trust ladder and add confidential projects ([dd832ad](https://github.com/abn/agent-hub/commit/dd832ad908b9d31bc2e9677c1ab2ac7a216165af))
* implement agent self-enrolment and pending token security invariant ([20eec65](https://github.com/abn/agent-hub/commit/20eec6516b9ea1d01dee8354330979b3724138bd))
* **inbox:** call a project by its display name on a row and the card ([1da18b2](https://github.com/abn/agent-hub/commit/1da18b2301949a70dae13bea9e960874ddc9979c))
* **inbox:** keep a decision's note as a field, cap it and hand it back ([a3cb81e](https://github.com/abn/agent-hub/commit/a3cb81e5d8741ff791ab8d231eafddca016b71cc))
* **inbox:** mark items read, one or all at once ([30a946a](https://github.com/abn/agent-hub/commit/30a946a39b9fc936dea6a289983db3eb6d470920))
* **inbox:** name an item's project on the item ([e78122f](https://github.com/abn/agent-hub/commit/e78122f1cf5470c05fdc33744c2ad161d05691a7))
* **inbox:** show resolved items in Earlier and support device-local snooze ([35066af](https://github.com/abn/agent-hub/commit/35066af8617874d66d864a9f0c4b13d52f0257f1))
* **inbox:** take an optional note with a decision ([fb7435f](https://github.com/abn/agent-hub/commit/fb7435f927d3b07e21766161e0a0d14d190b521f))
* **kb:** comment threads on knowledge base pages ([7d72c17](https://github.com/abn/agent-hub/commit/7d72c1771b283837ab22cbbb58dc5aee58763bea))
* **limits:** cap a knowledge base page at 1 MiB ([e902044](https://github.com/abn/agent-hub/commit/e9020440cbacf54323010cb6338f2b03a1ff6ace))
* **mcp:** deliver notifications on the tool result and by subscription ([753ac03](https://github.com/abn/agent-hub/commit/753ac03d6d8c369ee81e2f242ceb2b4e3334151f))
* **mcp:** let an agent wait for an answer, and poll from a cursor ([24cf4fa](https://github.com/abn/agent-hub/commit/24cf4fa24799f751cb4a2e574a0a50712f2a4059))
* **mcp:** report the predecessor's handoff when a session starts ([33f0769](https://github.com/abn/agent-hub/commit/33f0769004aed920d384ac61dc276a2be18c7f55))
* **mcp:** serve the agent guide as a resource and tidy the schemas ([12fa5a8](https://github.com/abn/agent-hub/commit/12fa5a8651c0ccf7746432225e7343ac8dbea927))
* **mcp:** serve the agent guide as a skill over MCP ([b56b29b](https://github.com/abn/agent-hub/commit/b56b29b4215719dcb9021b40b44201b4241f35ca))
* **mcp:** support embedded stdio mode with isolation ([6ae2d12](https://github.com/abn/agent-hub/commit/6ae2d126791ca94f36131ae48e386f49a4760e2d))
* **metrics:** count requests, events and tool calls, and answer an unknown tool ([1077916](https://github.com/abn/agent-hub/commit/1077916fa58d43db7eb6a29c8a0af776593b0143))
* **okf:** a line-oriented frontmatter reader and patcher ([7177c5c](https://github.com/abn/agent-hub/commit/7177c5c13ce60ab73c03e6a218bfbc063767d95f))
* **okf:** links, backlinks and lint ([6dff015](https://github.com/abn/agent-hub/commit/6dff0150568660296e0a14263193e5e5c0a768ed))
* **ops:** add the doctor command ([cf497f4](https://github.com/abn/agent-hub/commit/cf497f471e107442e5591c010ae7ad667998ecbc))
* **ops:** back up, restore and verify the store offline ([322fd12](https://github.com/abn/agent-hub/commit/322fd125d205e7ae728ac1891c745b2eb5c48228))
* **ops:** sample store integrity in the background and report it ([5bd086d](https://github.com/abn/agent-hub/commit/5bd086df9270144659044dbaa071ef6230db36cb))
* **projects:** count what a project holds ([9eefc14](https://github.com/abn/agent-hub/commit/9eefc14c9df0ee5da8912a7bad2f8a01860db755))
* **projects:** rename a project and set its artifact policy ([b11ee70](https://github.com/abn/agent-hub/commit/b11ee70d96e63ff763ffbb9694f761e46548d748))
* **search:** make knowledge base pages a corpus family ([a8252c3](https://github.com/abn/agent-hub/commit/a8252c37ec86a7d84f1f17add2244ae87ee41731))
* **search:** narrow a search to one session ([8ab6d6c](https://github.com/abn/agent-hub/commit/8ab6d6c5df29877dda698099d50037c295ed8674))
* **search:** report how many hits there were and how long it took ([465ab18](https://github.com/abn/agent-hub/commit/465ab182ca70418a1a596e8871c1896de7554577))
* **search:** say what a hit is on its row ([9ae6029](https://github.com/abn/agent-hub/commit/9ae6029361cb7d4fc95f852930bab8f46c052c54))
* **search:** say what kind of thing a hit is ([3099235](https://github.com/abn/agent-hub/commit/3099235954262ea93395553cd7956f40b80e12f2))
* **search:** support prefix matching with whole-word ranking ([b6dd215](https://github.com/abn/agent-hub/commit/b6dd215b5c33d1d4d7147a5f9a4ba06c0e1f8d53))
* **sessions:** count an agent as active while it works ([dd15410](https://github.com/abn/agent-hub/commit/dd15410ed09e85aa5c0cef909e55781ddc4c7b15))
* **sessions:** pick up another agent's session ([651f511](https://github.com/abn/agent-hub/commit/651f5113c61f286377185f7b17ddea65f969ec59))
* **sessions:** redraw sessions list and detail for round 3 ([f152cd2](https://github.com/abn/agent-hub/commit/f152cd2377fdd8baaa25fb80b1dfafb5ff384576))
* **sessions:** resume only the caller's own session ([bbee2b0](https://github.com/abn/agent-hub/commit/bbee2b0326084de14265a06a0df77d840fbd2bbe))
* **storage:** count knowledge base pages that need review ([eeeeb3c](https://github.com/abn/agent-hub/commit/eeeeb3c3a0eeb1fef8d97788251b1227a90a3319))
* **storage:** prune every ended session of a project ([95dc522](https://github.com/abn/agent-hub/commit/95dc522e5285785f9714028dc23f470d1afb1614))
* **storage:** report the volume, the kinds and what a prune frees ([e94d17a](https://github.com/abn/agent-hub/commit/e94d17a4367b382c79b329ca44b6058a4aa66590))
* **storage:** split a project row four ways and fold the empty ones ([158fc9b](https://github.com/abn/agent-hub/commit/158fc9bd2deca89fa2deb37b88dcf573f30b9f08))
* **storage:** split a project row four ways and keep an empty project listed ([85a9d57](https://github.com/abn/agent-hub/commit/85a9d57c69290de9d0cdbaac05967b810b59f9c7))
* **store:** cap the events one project may hold ([3e06d13](https://github.com/abn/agent-hub/commit/3e06d13586f836e62d0a0ce68ddb028452a89e00))
* **store:** carry the artifact policy and the feed cursor ([35967e9](https://github.com/abn/agent-hub/commit/35967e97d11ff9df08434248bda127f001453070))
* **store:** count knowledge bases in storage usage ([a59266b](https://github.com/abn/agent-hub/commit/a59266b4032ba5b70da4903bf0ec1cdaef25433b))
* **store:** keep ids increasing across a clock jump, drain on SIGTERM, health for containers ([ccb3c0a](https://github.com/abn/agent-hub/commit/ccb3c0ad2070feeda7f9737b52d92b12d1bbf843))
* **store:** key a session by the agent that owns it ([757c46d](https://github.com/abn/agent-hub/commit/757c46dfa999035e0db8dd83885d6ecb396aff26))
* **store:** name the session an event belongs to ([2189ab0](https://github.com/abn/agent-hub/commit/2189ab0f3774be76d4e2bdbc3c75b99adfeb0f4b))
* **store:** record artifact publishing actor and surface across mcp and rest ([4178a46](https://github.com/abn/agent-hub/commit/4178a46d35e85e414b642018b5726b16a1fa9a8b))
* **store:** record caller session lineage on artifacts and filter feed ([123483b](https://github.com/abn/agent-hub/commit/123483b1e7597f66b721acecf4818ee5810fa06f))
* **store:** remove a knowledge base with its project ([175f643](https://github.com/abn/agent-hub/commit/175f6430545d46e822c8130796458bc3f31cbd82))
* **web:** a project's own settings screen ([d554094](https://github.com/abn/agent-hub/commit/d554094d087ba2771f0cf18f6958b4e2621abf8c))
* **web:** a session's brain as a tree, and a designed detail ([b25436e](https://github.com/abn/agent-hub/commit/b25436ed269cbc7db62cc75aabc4d1aae855ac8c))
* **web:** add agent button trailing in desktop header ([aa7fda9](https://github.com/abn/agent-hub/commit/aa7fda9579df4f4c2022fc782a5351723a18b54a))
* **web:** add desktop search layout with preview stage and match stepping ([7ae445e](https://github.com/abn/agent-hub/commit/7ae445eb3cd30f51151ddf666aaa2eb0aa4075aa))
* **web:** add phone inbox and search tools rows with flat rows and single search title ([7af824f](https://github.com/abn/agent-hub/commit/7af824f58b5b5aca30651b61a857e6c23991f179))
* **web:** add project tools row, brain entry stage, and artifact agent grouping ([9552f07](https://github.com/abn/agent-hub/commit/9552f0725e73f58d009435dd9536e5464470c7cd))
* **web:** add the knowledge tone and the desktop row form for round 12 ([4586125](https://github.com/abn/agent-hub/commit/4586125f33c9ecb9ac2b89a84a6d9c56f5bfa2a7))
* **web:** add version row and simplify storage row value in settings ([97fe15b](https://github.com/abn/agent-hub/commit/97fe15bbf45aba83cfbf0ab21bb66f56217a99bf))
* **web:** adopt radius hierarchy, fixed trigger labels, and glyph additions ([af07e32](https://github.com/abn/agent-hub/commit/af07e32542ee1625acd71e4f9b1d9240d1571fc2))
* **web:** an arrow on the reply send control, dimmed when there is nothing to send ([abb7d27](https://github.com/abn/agent-hub/commit/abb7d27f4328354241e03e0da68022db08636350))
* **web:** an empty state that says what this is and what to do ([e6df2ed](https://github.com/abn/agent-hub/commit/e6df2ed9335762559484abb69d2472f38f8f3066))
* **web:** an inbox that can be read, swiped and refreshed ([48883ce](https://github.com/abn/agent-hub/commit/48883ce8602af543cc4fdfc5ceae638080664ef6))
* **web:** app rail, layout zones, and prose measure ([cbcd099](https://github.com/abn/agent-hub/commit/cbcd0990f83bb448717de70882694aa952d0acbc))
* **web:** ask for the access token on a screen of its own ([ac0d850](https://github.com/abn/agent-hub/commit/ac0d8509dc9def8edc4ba51c26048fe25cbf1df4))
* **web:** bring agents and tokens into the settings frame ([046b46e](https://github.com/abn/agent-hub/commit/046b46e5dc33177cb786091e975bffdc1ebf8a92))
* **web:** bring desktop settings to the round 11 row form ([5622b63](https://github.com/abn/agent-hub/commit/5622b63ac31a0e8a446ec104f32e4ab7253bfb13))
* **web:** bring desktop storage to the four-segment bar and a column per kind ([db6fb4f](https://github.com/abn/agent-hub/commit/db6fb4ffaa926d8a29b3853f0fd3d270bf1b3b34))
* **web:** bring projects register and connect onto phone shell ([4a4665a](https://github.com/abn/agent-hub/commit/4a4665a0b9d2a8acbe1b13ac7f7e56a93919332a))
* **web:** call a project by its display name ([159ebb6](https://github.com/abn/agent-hub/commit/159ebb6b0b3500c56bf939b11d0b1f45b860f22e))
* **web:** confirm and report in the app, not the browser ([dc1a0de](https://github.com/abn/agent-hub/commit/dc1a0de81b6fd1720b4376829a165797680538e6))
* **web:** desktop artifacts gallery 5-up grid and three-pane viewer ([8a22f01](https://github.com/abn/agent-hub/commit/8a22f017260fbb80b218a95ad367b80318171695))
* **web:** desktop four-zone layout for sessions and brain file viewer ([8d05bdf](https://github.com/abn/agent-hub/commit/8d05bdfd3e7eea60253df6af55f07af86d488a5d))
* **web:** desktop home layout and inline waiting actions ([5fadf67](https://github.com/abn/agent-hub/commit/5fadf67ba042461eb5f582fc3fdd9b66c0173c67))
* **web:** desktop project screen layout and live state aside ([eee759a](https://github.com/abn/agent-hub/commit/eee759ac47a5e52368cd890f7d20e6f037887551))
* **web:** desktop storage multi-column table and summary tiles ([4322f84](https://github.com/abn/agent-hub/commit/4322f842bed08bdb66ea0c01fb3f63018cc8e4b7))
* **web:** frontmatter reader and patcher for the browser ([e324b2c](https://github.com/abn/agent-hub/commit/e324b2cb7304b78d885ee255be5a8d35bd3c46c1))
* **web:** give the desktop register the filter field instead of the switcher ([14ddedc](https://github.com/abn/agent-hub/commit/14ddedc23a70738ff3bf6d3c5937a61389ae4eda))
* **web:** grouped settings layout and segmented controls ([ea5ab5d](https://github.com/abn/agent-hub/commit/ea5ab5dc99cd443199004c9525510aebf6140d89))
* **web:** Home as the day at a glance ([896d231](https://github.com/abn/agent-hub/commit/896d231ec3a55e7057b26ce9952973af8a9b9cad))
* **web:** implement phone frame, tools row, and more tab ([7facc74](https://github.com/abn/agent-hub/commit/7facc74f10e55af1703dbdf6e695b6b8e477bfdd))
* **web:** inbox header carries mark all read and overflow with refresh ([dfa3b3e](https://github.com/abn/agent-hub/commit/dfa3b3e29efb5cd5b5af1d3b945b3a06f8aba503))
* **web:** make agents and tokens list and item on desktop ([fb50b5b](https://github.com/abn/agent-hub/commit/fb50b5bc1e334321bc9450e8156c9d34b53cf8a8))
* **web:** make the first run work on desktop and phone ([3341ea6](https://github.com/abn/agent-hub/commit/3341ea6b6c62ec0865436c81fdbcb1063ab900e3))
* **web:** mobile flat rows for settings, storage bar with knowledge, and token-safe agents screen ([629eb72](https://github.com/abn/agent-hub/commit/629eb72d344ea943062cac5aac9fc2ae63018b7e))
* **web:** name the node in the installed app ([64e0dbd](https://github.com/abn/agent-hub/commit/64e0dbd74fda01862a69ccc47ffe4aaabb337eac))
* **web:** one composer, the send control inside it, and a way out of a thread ([0769ebc](https://github.com/abn/agent-hub/commit/0769ebc7c11b7fec2eb91844a66853f58e758d66))
* **web:** one keyboard map for the screens with rows ([13e9e22](https://github.com/abn/agent-hub/commit/13e9e22996bc8379093e4ab25919d101708c1127))
* **web:** open add agent form in stage on desktop ([129376f](https://github.com/abn/agent-hub/commit/129376ff457bb2017a0cadf8e878dce8e467b41d))
* **web:** project deletion with typed confirmation and count manifest ([088c206](https://github.com/abn/agent-hub/commit/088c206546c81e76dac1216a66b22828e824d614))
* **web:** put Home back to a screen header and keep the greeting in the stage ([08a684d](https://github.com/abn/agent-hub/commit/08a684d052a4fe3c0911ac1202e4de2737c9f2b3))
* **web:** put the type scale and the controls on spec ([799fde2](https://github.com/abn/agent-hub/commit/799fde281dd64dbf89ca97a51ad605c8beb66857))
* **web:** reassign a session from its detail header ([dbeaf59](https://github.com/abn/agent-hub/commit/dbeaf59348e8cdb39b30a3ecab27687e4b77dd52))
* **web:** recent changes pages back in place ([0016770](https://github.com/abn/agent-hub/commit/001677008db1b5fe0c095b8575098a8faf65f317))
* **web:** relocate settings gear to home and add project creation sheet ([3356381](https://github.com/abn/agent-hub/commit/33563811e11dff44cbeb887b2d394462139299a7))
* **web:** render projects index screen at #/projects ([8d3d99a](https://github.com/abn/agent-hub/commit/8d3d99addc3045d216660ff1acfa8df03af04aa5))
* **web:** restore agent creation and project grant access capabilities ([0c73193](https://github.com/abn/agent-hub/commit/0c73193b6fe92c2cf3ff3d5e7e530bb30d1c4014))
* **web:** retire storage tiles and move prune all to control row ([7846b89](https://github.com/abn/agent-hub/commit/7846b8970d50675387c1c60d751d1aedf838b3e3))
* **web:** reveal a reissued token in one dialog with two states ([194b871](https://github.com/abn/agent-hub/commit/194b8713f8bbdbd4a76b7559d6f7976251e3c200))
* **web:** round 14 conformance across access, shares and glyphs ([c226753](https://github.com/abn/agent-hub/commit/c22675388f634d39509c670dba59f17e6f598f8c))
* **web:** search is the same shell ([c0214e3](https://github.com/abn/agent-hub/commit/c0214e3ab62d374d72efa9343929a1e5457c27fb))
* **web:** search that answers as you type ([219bed1](https://github.com/abn/agent-hub/commit/219bed1edbf8022af338a66f077e6ce8f30411a9))
* **web:** settings is rail and stage, with no index ([a049e7f](https://github.com/abn/agent-hub/commit/a049e7fa4959aefeca3172deae5c248a251ee161))
* **web:** show sync only when unhealthy, on both widths ([0658826](https://github.com/abn/agent-hub/commit/0658826b97b4e0e8466f26d16451d255373c2b5b))
* **web:** show the rail's sync line only when it is stale or failed ([72d05ac](https://github.com/abn/agent-hub/commit/72d05ac19e24cce9c6e5bed5b4ca20b355b10dcf))
* **web:** show time short, with the whole stamp on press ([1322b14](https://github.com/abn/agent-hub/commit/1322b14023f0a72ba05c2028d0b95e08b4aae2bb))
* **web:** standalone access screen and binary grant surface ([018de94](https://github.com/abn/agent-hub/commit/018de9442c1822c12e6e20c9ec0ed3e8cd6eae34))
* **web:** the artifact reads in the stage ([d6c09be](https://github.com/abn/agent-hub/commit/d6c09be8ccd08a1dee107ff153f0da8aa6feede1))
* **web:** the artifacts index groups, and the group is a value ([63e5b14](https://github.com/abn/agent-hub/commit/63e5b143a774e959b027b14d6ff5d0e7fd6f4a81))
* **web:** the comments are the aside on a fine pointer ([09f60c4](https://github.com/abn/agent-hub/commit/09f60c40c51058d224ba2d2fbce7ffef35619041))
* **web:** the designer's four answers from round 8 ([f1d7a6b](https://github.com/abn/agent-hub/commit/f1d7a6bde813d83c0872239de6398d19cca9da26))
* **web:** the feed's kind filter folds into the Group menu ([04e7f7f](https://github.com/abn/agent-hub/commit/04e7f7f85536da58de5496e7978994aa661ed691))
* **web:** the inbox is the same shell ([8df667f](https://github.com/abn/agent-hub/commit/8df667f72178087454832df94ee6ff886031daf3))
* **web:** the inbox rows take the design's scannable shape ([eeac025](https://github.com/abn/agent-hub/commit/eeac02554014433b37998e1fb6e0ab689f2f5a07))
* **web:** the phone bar and its 48px leading slot ([d49edc2](https://github.com/abn/agent-hub/commit/d49edc2b128ce0711c06f74a47be56bba10f7aba))
* **web:** the project feed to its design ([6125346](https://github.com/abn/agent-hub/commit/612534610748ab53a358d888501f3a5d164fc3e4))
* **web:** the project view in the one shell ([2302f53](https://github.com/abn/agent-hub/commit/2302f53f14da74d20beaf4fb5060cd420377c776))
* **web:** the project wiki, reading and writing pages ([db76c64](https://github.com/abn/agent-hub/commit/db76c64e78971fa01a862c7e295b438f0fa62fd8))
* **web:** the shell, the project segments, the gallery and the viewer route ([cca968f](https://github.com/abn/agent-hub/commit/cca968fb6a9e4165e9271a92ae39b5366c586467))
* **web:** the storage screen, with prune by project and prune all ([55d0fdd](https://github.com/abn/agent-hub/commit/55d0fdd18de0f6ff35b7613efbbff1d84abcb9fb))
* **web:** the wiki home, recent changes and lint ([eaf1bd6](https://github.com/abn/agent-hub/commit/eaf1bd6ed60b7fdd9ceff03e671334ead31bd0c3))
* **web:** update settings groups to this device, keyboard and this hub ([bef38c6](https://github.com/abn/agent-hub/commit/bef38c69ba5a5b2afc5c2f49a05880bb43edb3fe))
* **web:** welcome-first home layout and flat rows on phone ([1506e25](https://github.com/abn/agent-hub/commit/1506e25ee39996ed5e6ff1c6391abe98576cd09c))
* **web:** wiki comment threads, the page sheet and the project header overflow ([c1945c7](https://github.com/abn/agent-hub/commit/c1945c7c5c42d5c613c78864884f138790f48219))
* **web:** wiki reviews, save-to-wiki and the editor conflict path ([df66d4e](https://github.com/abn/agent-hub/commit/df66d4e4a6d021079bc73847676db71769e90fbd))


### Bug Fixes

* **api:** answer the share link and the project listing with what the hub knows ([597637e](https://github.com/abn/agent-hub/commit/597637e980ed435f8f8d67da4f4c036a6ea36d5e))
* **app:** log the address the hub listens on, not the one configured ([c5c8aec](https://github.com/abn/agent-hub/commit/c5c8aecc16f1dd2231e9eb1ee468046ac8fdd4f1))
* **artifacts:** allow an update to clear a label ([aff7523](https://github.com/abn/agent-hub/commit/aff75239ba34d30574dc0bd50e43071cc4094837))
* **artifacts:** consolidate markdown rendering onto engine ([c3292f0](https://github.com/abn/agent-hub/commit/c3292f0da6a05b111864e6da0343fe9989216a20))
* **artifacts:** guard hidden viewer menus and default to newest version ([59a5a6e](https://github.com/abn/agent-hub/commit/59a5a6e351dba120fc99998f5aaefd0bf5bb1c64))
* **artifacts:** list the kinds a publish accepts in its schema ([46cf7e2](https://github.com/abn/agent-hub/commit/46cf7e20714fc85125108d0da04ab85e85757b74))
* **artifacts:** make the locked gate fit, and let the password be read ([7873e94](https://github.com/abn/agent-hub/commit/7873e94ecc3254523c98bd20ac3f9c9637a96493))
* **artifacts:** one comment box, and hold hidden to meaning hidden ([c85eeed](https://github.com/abn/agent-hub/commit/c85eeed127acdaa2c3b786bee5bf03a61c85c528))
* **artifacts:** put the comment count beside the bubble, at a legible size ([b69151c](https://github.com/abn/agent-hub/commit/b69151cb5a3bad6cd65d4b51168cfadf8f39c62f))
* **artifacts:** reach a live-shared artifact from the owner's viewer ([46057b1](https://github.com/abn/agent-hub/commit/46057b1ccb77ed8beade841ab54f9a5128e5cae7))
* **artifacts:** render public markdown with the parser that can read it ([96cbc50](https://github.com/abn/agent-hub/commit/96cbc50865e1a3109597816293b6df44aa31b662))
* **blob:** clean up promoted files on update failure and reconcile on-disk version blobs at startup ([6e9a3b3](https://github.com/abn/agent-hub/commit/6e9a3b3fe9d524713b0c00b45df695666d570b31))
* **brain:** reconcile orphan session files and recover the lock table ([3164a8b](https://github.com/abn/agent-hub/commit/3164a8bea8ddc5e093bb59c2a40930741f8135b1))
* **build:** carry the build script into the image and accept the commit as an argument ([993993b](https://github.com/abn/agent-hub/commit/993993b1f7a856396d0a5c792050ec1fc579ec61))
* **build:** reinstall the Node tree so a new dependency is picked up ([5fa1312](https://github.com/abn/agent-hub/commit/5fa131200100e2487c2485949266bc9c8311220c))
* cap a comment anchor quote, and build the image in CI ([a062023](https://github.com/abn/agent-hub/commit/a0620230a21ae3c3131b20564500c1e9ede66bf6))
* **ci:** validate the docs bundle with okf in the pages workflow ([3b53780](https://github.com/abn/agent-hub/commit/3b5378069727de63b22dad6f8e9231cc13260d5e))
* **cli:** drop the duplicate no-client tools entry ([1e1ef0f](https://github.com/abn/agent-hub/commit/1e1ef0f77d70651c711e3801ae90f511bffcef26))
* **client:** keep a successful call's stderr free of the transport's teardown line ([12c17ed](https://github.com/abn/agent-hub/commit/12c17ed3acb2bc0618a25c7417d9f64668e96ac3))
* **client:** keep HUB_PROJECT off a session-store call ([baac255](https://github.com/abn/agent-hub/commit/baac255eb41907d43ba76c8bdab93fa909bcc9c0))
* **client:** serve MCP resources over the stdio proxy, and split exit codes ([e0fc25d](https://github.com/abn/agent-hub/commit/e0fc25d454edbf7113f6e511d8f2885c3f40d206))
* **cli:** every subcommand answers a help flag with its usage ([afa8577](https://github.com/abn/agent-hub/commit/afa85776bed057592bdd2ea7b67bfdade551c56e))
* **cli:** list every page a knowledge base holds ([3b13cdb](https://github.com/abn/agent-hub/commit/3b13cdb6bd0f9ea71023efd3f2b9a1768aef3024))
* **cli:** reach project lifecycle without curl ([ae4d6d9](https://github.com/abn/agent-hub/commit/ae4d6d9541355bd3db59657a858a2aad15035759))
* **comments:** anchor the stage's comments to the artifact it shows ([a259ff6](https://github.com/abn/agent-hub/commit/a259ff63bd51b11f4b7db52138314efee7483057))
* **comments:** mount the drawer on a fine pointer so the stage callout works ([b04876d](https://github.com/abn/agent-hub/commit/b04876dd894f16406d3b4a4cbe9d175473ac1411))
* **config:** honour the documented keys and fail on an unknown one ([4f4b3f2](https://github.com/abn/agent-hub/commit/4f4b3f20dc69cb26ef49f5dc879612e428457412))
* **config:** resolve the data directory without the serve credential ([560586e](https://github.com/abn/agent-hub/commit/560586e094caead6be6f2a028f927670f06a7163))
* **config:** write the client token atomically and owner-only ([7491bd0](https://github.com/abn/agent-hub/commit/7491bd0110ad1ee5a3b84f3d096b19326bc21f63))
* **container:** copy the served skill into the build ([58430b4](https://github.com/abn/agent-hub/commit/58430b4c0d558c36e0cce0471b61c6e08cc65e06))
* **container:** copy the skill directory and name images fully qualified ([bc71695](https://github.com/abn/agent-hub/commit/bc7169537c67300bcd2d980edad9fa1cf536837b))
* **e2e:** keep concurrent runs from reaping each other's hubs ([bb79e76](https://github.com/abn/agent-hub/commit/bb79e761bb3b67a076f68212769bcd817febeee7))
* **enrol:** decide a self-enrolment through the approval decision ([886bbf0](https://github.com/abn/agent-hub/commit/886bbf0a269a516ecbecd3be9663a230b55f0dbb))
* **enrol:** identify an enrolment by its socket peer and bound pending requests ([6c2f3dd](https://github.com/abn/agent-hub/commit/6c2f3dd473b8e3594a894b61b141d41cb08e2e59))
* **feed:** a thread id names the start of a thread, and a replay skips the check ([5a231ff](https://github.com/abn/agent-hub/commit/5a231ff6a9a18ec9659860d8b032fe2902a453a9))
* **feed:** capitalize kind filter chips and wrap across lines ([a9ca45d](https://github.com/abn/agent-hub/commit/a9ca45d29f8af8434f7230f2270eb91a351e548a))
* **feed:** validate thread root exists and belongs to project ([a4c2111](https://github.com/abn/agent-hub/commit/a4c211162d3e8cbb1cca1044c3f078711689308b))
* **harness:** give the hub longer to start and to stop ([9aae17f](https://github.com/abn/agent-hub/commit/9aae17fc1e91db9bd0d4bd53583d6af2f4868bac))
* **home:** draw the storage bar to scale, or not at all under 1% ([f673970](https://github.com/abn/agent-hub/commit/f6739709611e2183225389f527d6c47f436e5742))
* **http:** unify query percent decoding with form_urlencoded ([c96a8c0](https://github.com/abn/agent-hub/commit/c96a8c0f9b612606403e2192d094b4aa4cc68f89))
* **inbox:** attach answers to resolved questions in inbox listings ([00e8c18](https://github.com/abn/agent-hub/commit/00e8c1870efaf10e7e16ff84efa1459444cb5917))
* **inbox:** close the card by its control the way Esc does ([08b5c36](https://github.com/abn/agent-hub/commit/08b5c36d239187e084bbb3874decbd17f7ba8c6b))
* **inbox:** give the desktop card a close control, and let Esc close it ([5ca79b6](https://github.com/abn/agent-hub/commit/5ca79b632c09ddf9d5a94d3e800caaf5af4f5087))
* **inbox:** hand focus to Earlier when the closed card's row is folded ([f1c2334](https://github.com/abn/agent-hub/commit/f1c2334a30c08e965988c2de27203feafdbb921c))
* **kb:** a review's own write is not an edit since the review ([d0a45fe](https://github.com/abn/agent-hub/commit/d0a45fec2911f0dd5d3a1e4057b0f6508f8f6214))
* **kb:** edited since review compares what was stored, not when ([2cb41cb](https://github.com/abn/agent-hub/commit/2cb41cb486adcb86cb0e5389663c6501080ac195))
* **keys:** the selection follows focus into a row ([2224459](https://github.com/abn/agent-hub/commit/222445950ca22d11cbe66b6708fbba138aef109b))
* **mcp:** let a session-store write name the caller's own session ([b7f0221](https://github.com/abn/agent-hub/commit/b7f02211d8eef225aefbc453d307a064ab87fd4b))
* **mcp:** map a token-gate error to its real status, and count codes ([3244360](https://github.com/abn/agent-hub/commit/32443603521429c233a68b0184410acfc8daa638))
* **mcp:** name the project store's namespace in a path refusal ([0e4b144](https://github.com/abn/agent-hub/commit/0e4b1443774e855631c64440fc8cce21dfff9b1c))
* **net:** rebuild the tailnet device on a full session drop ([71fdc83](https://github.com/abn/agent-hub/commit/71fdc832a075748d458bc93e6025f462460f24c8))
* **okf:** a link inside code is not a link, and a page is not its own referrer ([8529900](https://github.com/abn/agent-hub/commit/85299007685e4beb1c66e4d41cead4e0c7ee2203))
* **okf:** escape U+FFFE and U+FFFF in a written frontmatter value ([bf400f8](https://github.com/abn/agent-hub/commit/bf400f804293f856d1b021ed4cbe9080edbb5fcb))
* **okf:** refuse a first line that only looks like the opener ([3aa7416](https://github.com/abn/agent-hub/commit/3aa74160f01b081753d7711ddebc53ab8d036b09))
* **okf:** refuse a list or mapping inside a record as an invalid value ([f5a44fd](https://github.com/abn/agent-hub/commit/f5a44fd7c6ca94700326324ca8e58d525bdf5118))
* **onboarding:** name the config remedies and the minimum frontmatter ([d386ebf](https://github.com/abn/agent-hub/commit/d386ebf0d72df6e24ad5c3d1c7554feeef42b45a))
* **ops:** diagnose an older store, back up the tailnet key, run offline ([c1d47d1](https://github.com/abn/agent-hub/commit/c1d47d19a80af932bc8e92af941a5254e0a3c5eb))
* **projects:** remove a deleted project's knowledge base directory ([c416a4e](https://github.com/abn/agent-hub/commit/c416a4ef946b63e40b64e4b7e46e7d96fd37762e))
* **release:** point the verify step at the served bootstrap ([7d5806d](https://github.com/abn/agent-hub/commit/7d5806ddcdd6e951311562e9513c4756012b4dd0))
* **router:** a bare project address left behind keeps off the route ([8b0e966](https://github.com/abn/agent-hub/commit/8b0e966d1611ed3936905f62c5ebc4fc16113887))
* **schema:** renumber the review migrations to 15 and 16 after the grant rebuild ([ada752c](https://github.com/abn/agent-hub/commit/ada752cbd15a6410af75a9c609bbe9495597c112))
* **scripts:** clear the data directory the capture actually opens ([efef12c](https://github.com/abn/agent-hub/commit/efef12caaa4dab3371060ebdc42ca29225e03d09))
* **scripts:** point the harness importers and the skill root at the new layout ([a1cfa2a](https://github.com/abn/agent-hub/commit/a1cfa2a4eb8a5a261ea51a48c76510767df0738d))
* **search:** enforce search body truncation and locked session indexing ([1fc505e](https://github.com/abn/agent-hub/commit/1fc505e51411855b388addc4d9f60400c8be1579))
* **search:** sanitize FTS query input and handle hostile strings ([4aae88a](https://github.com/abn/agent-hub/commit/4aae88a650e89a8928898dddf2f01e2f91a6ddae))
* **search:** send the query as typed ([0f3002a](https://github.com/abn/agent-hub/commit/0f3002a9679d877ff07170efb7e00e0b32e9d18c))
* **search:** show what the agent wrote in a feed snippet, not payload JSON ([095b28e](https://github.com/abn/agent-hub/commit/095b28eae83b97c0ce77e1506292fbb714034774))
* **sessions:** checkpoint a brain when its session ends ([c2e3053](https://github.com/abn/agent-hub/commit/c2e3053ee13a5a34f19822ddfcbcf955e1950f71))
* **sessions:** enforce guarded ownership transitions and locked brain activity validation ([6a15fc4](https://github.com/abn/agent-hub/commit/6a15fc49c3b958a3cbe33c119f89f529665b20e5))
* **sessions:** fold finished brains into their file at startup too ([537dc6d](https://github.com/abn/agent-hub/commit/537dc6d5ffd00d6d8b2256c483087a844f7d03e0))
* **sessions:** open a session inside its project, keep the tree focus ([82c1e18](https://github.com/abn/agent-hub/commit/82c1e18b71260b4e3a7fc58e710b4e55947c7f26))
* **settings:** take the token only on the screen that checks it ([be7d6c4](https://github.com/abn/agent-hub/commit/be7d6c468a615ed1321bdd483b2235446b0aa110))
* **shell:** the slash key focuses the Search screen's own field ([856f217](https://github.com/abn/agent-hub/commit/856f217adc632c72d9ac7e06e914ab933f957648))
* **storage:** draw a small hub's summary bar against what is used ([a6e87d9](https://github.com/abn/agent-hub/commit/a6e87d9ef4cb79804815c60621b51f6048d79437))
* **store:** a grant is access, and an agent may lock a project ([6b3102a](https://github.com/abn/agent-hub/commit/6b3102ac739d77c780b12679cc0981e6e69dcf62))
* **store:** bound the feed's agent-content writers by the ceiling ([992c8b1](https://github.com/abn/agent-hub/commit/992c8b1228b98a88d6177eb101e5e82bf0b4b21d))
* **store:** clear a comment's idempotency row on delete ([2d1c258](https://github.com/abn/agent-hub/commit/2d1c2582dcae6cd326733811dfd30562228970a6))
* **store:** count only a live share when concealing an artifact's page ([88b4bbf](https://github.com/abn/agent-hub/commit/88b4bbfede2c6fb659234e566fe3fab7d4a2cd4f))
* **store:** enforce project write barrier and generation-scoped deletion ([96537b3](https://github.com/abn/agent-hub/commit/96537b332c7c04c48ba06ff6395667adae3f8497))
* **store:** keep the data directory and store private to the operator ([c4c2556](https://github.com/abn/agent-hub/commit/c4c255678880f07295167ad9b18820b9475ae226))
* **store:** leave a promoted blob to reconcile, and name a missing version ([d7171a2](https://github.com/abn/agent-hub/commit/d7171a2c15f4000a3552c729928ccc7d7796db49))
* **store:** move an agent's grants when its id is renamed ([4118055](https://github.com/abn/agent-hub/commit/41180550189c2a6315bc20621e567662c5ba7d4e))
* **store:** namespace idempotency operations and bind keys to target entities ([6eb31e5](https://github.com/abn/agent-hub/commit/6eb31e5735f5909c866648387421cd2d6a7850a1))
* **store:** refuse a newer store, back up before migrating, mean the readiness probe ([6e0f30a](https://github.com/abn/agent-hub/commit/6e0f30a04b302d787a5f3a3ee111b8fa9b74da58))
* **store:** serialize prune sweep and undo and exclude soft-pruned brain from search ([fc2e5cf](https://github.com/abn/agent-hub/commit/fc2e5cfa9bea7c0a3321ae454d081421458ce41c))
* surface a serving hub's failures, seed ids from the data, check the data dir ([3dbedab](https://github.com/abn/agent-hub/commit/3dbedab7699dfdcb806af0ad9120828d874acf8b))
* **tests:** keep the suite off the user's client config ([1bd0f76](https://github.com/abn/agent-hub/commit/1bd0f76744e5ed3b1f246160a461ede2de76bed3))
* **viewer:** draw one theme glyph, for the theme a press switches to ([e3e43e6](https://github.com/abn/agent-hub/commit/e3e43e6e3ce2086bf2fc20d6bc94d4d0fdf56d7a))
* **web:** a kind is a shape and a word, not a colour ([3315069](https://github.com/abn/agent-hub/commit/331506951f2eb970d702e29e97cb995413f4a964))
* **web:** a real table on desktop, a list on the phone, a readable preview ([94764ee](https://github.com/abn/agent-hub/commit/94764eea096238ddec15c8707241be4a31f7c152))
* **web:** a session opens its detail again ([f9e7868](https://github.com/abn/agent-hub/commit/f9e78688b81f1fdd0285fe06dad63614824744c9))
* **web:** a wiki directory row never addresses a page ([6f8e8e0](https://github.com/abn/agent-hub/commit/6f8e8e0c18e21777ddd2502b7ba153584e207dd3))
* **web:** advance the feed read cursor on the feed body, not a chip row ([01ec1fe](https://github.com/abn/agent-hub/commit/01ec1fee0feb3712950830314ec06e2c8fa8aaac))
* **web:** announce the search match position ([5596dc1](https://github.com/abn/agent-hub/commit/5596dc1bbad3c953a3f095136067b9d43fe91f32))
* **web:** artifact cards show a title and a preview worth reading ([ed5f6ba](https://github.com/abn/agent-hub/commit/ed5f6ba34639aa02830ec3aebda14b210c6c4789))
* **web:** confirm before ending a session ([12c29ed](https://github.com/abn/agent-hub/commit/12c29edef7adcf7083d5b44b6726242362e1e669))
* **web:** decide an approval once, however often it is pressed ([bb69ded](https://github.com/abn/agent-hub/commit/bb69dedfe83fa0f86fcefef783301411b82e0c23))
* **web:** dialog list text that meets contrast in light ([7cba321](https://github.com/abn/agent-hub/commit/7cba321d38911102e84b5b6eb44d4e9fe887665c))
* **web:** draw the focus ring once and space the Connect form ([f07d206](https://github.com/abn/agent-hub/commit/f07d206ad55f20aa40a6e16834f2a65b989728a5))
* **web:** drop board annotations and keep the settings agents label whole ([266fe34](https://github.com/abn/agent-hub/commit/266fe342837ee794542027c8cba5b9f221f42878))
* **web:** end a long summary's subject on a clause boundary, and keep the share field's ring in forced colours ([6dacb8b](https://github.com/abn/agent-hub/commit/6dacb8b1e102eaaabe5217d47d356af477a85049))
* **web:** follow the system theme while the app is open ([13ec17b](https://github.com/abn/agent-hub/commit/13ec17bc3c53b77e875b5d0537f43fa2f98b30bf))
* **web:** give the phone project screen one header and one tools row ([85b1d8c](https://github.com/abn/agent-hub/commit/85b1d8c91de1f433b397663dc1c6edc39d6e5b0e))
* **web:** guard the older-feed loader against an overlapping call ([f0f6201](https://github.com/abn/agent-hub/commit/f0f62011650e57523027b530df822bf29bfe3116))
* **web:** hang the viewer menu from its trigger and show one sheet head ([de25e12](https://github.com/abn/agent-hub/commit/de25e120b71017184955eb6c7e54248118ac8e9d))
* **web:** hide a filtered-out row so the e2e filter checks hold ([035cccb](https://github.com/abn/agent-hub/commit/035cccbebac4b0d74669f92d004724408a95c39a))
* **web:** hide More's sync row outright when healthy ([c4c36ec](https://github.com/abn/agent-hub/commit/c4c36ec7284fa506eb58906e88019cecdd789a87))
* **web:** hold the index gutter by the card's text, not a button's inset label ([3cbff69](https://github.com/abn/agent-hub/commit/3cbff6951f762faf25e40214db08a008e7dcdd1f))
* **web:** honour the waiting-on-you opt-out before notifying ([da6cc8a](https://github.com/abn/agent-hub/commit/da6cc8a62a798c5f1468a671c028f03d523bc6ab))
* **web:** keep a problem's status and code on the error ([ea0775a](https://github.com/abn/agent-hub/commit/ea0775a66cda4a492b72942201a4658b500d4b9c))
* **web:** keep a way into the list when the remembered row is gone ([c4cf013](https://github.com/abn/agent-hub/commit/c4cf013c21e0e6536764e66c9bd1e001caa409c6))
* **web:** keep an entry's state when only its address changes ([5294ecc](https://github.com/abn/agent-hub/commit/5294ecc95a7653b50ba9b25576f8fc16cd3e5ebf))
* **web:** keep the About row honest and give the version one source ([7b4807f](https://github.com/abn/agent-hub/commit/7b4807f543f6028c6a4771c16a27f9be793caeb2))
* **web:** keep the focus ring where shadows are dropped ([4de9fc5](https://github.com/abn/agent-hub/commit/4de9fc5eb010ebfb202a58102f56f76d56a59101))
* **web:** keep the hub's reason for refusing a request ([8b0d82b](https://github.com/abn/agent-hub/commit/8b0d82bcef79a2ca7af597f36a9b8c0b4caeb465))
* **web:** keep the status pill on one line and show a KB hit's display path ([8d58829](https://github.com/abn/agent-hub/commit/8d5882987ea38995758aeb983c599685178486dc))
* **web:** keep the theme control on the design's 34px track ([b96bd37](https://github.com/abn/agent-hub/commit/b96bd37957e2cbe93c38b986fbccac35a103ab22))
* **web:** layer tab bar, route mobile settings, and serve install icons ([21465f3](https://github.com/abn/agent-hub/commit/21465f3bf621ab95a7dd8cf198c5224d183cb3f8))
* **web:** lead the agents control row with the confidential count ([3ec97d8](https://github.com/abn/agent-hub/commit/3ec97d8ff0b787070037273dd14b8ec543c811a1))
* **web:** leave the route alone once the reader has left search ([fe4e9a5](https://github.com/abn/agent-hub/commit/fe4e9a580f844670021afd19ba0cc0494dde3a3a))
* **web:** make the artifact stage version control open the version sheet ([18dfe2a](https://github.com/abn/agent-hub/commit/18dfe2a91d88bc379965dcd9456dcd288b4d26c5))
* **web:** make the comments panel header show the live open count ([9e487a3](https://github.com/abn/agent-hub/commit/9e487a3619a7295215bb04470d3973d143c94520))
* **web:** make the phone home header the greeting, and keep it in flow ([f7a2e8f](https://github.com/abn/agent-hub/commit/f7a2e8fdb278f6b728806e8a23d2b36d06eae420))
* **web:** offer remembering a password only where it works ([fae0a49](https://github.com/abn/agent-hub/commit/fae0a49b1a02897d38cd9b7c6b627b0d78f6d111))
* **web:** one palette, checked in both themes ([a6d53d7](https://github.com/abn/agent-hub/commit/a6d53d7eeb2f429b69743fe915db63b0c19c02a5))
* **web:** open an inbox item from anywhere on its row ([8e0bd3a](https://github.com/abn/agent-hub/commit/8e0bd3aedb36b8e3da9b03f787b69ad01b1db9b7))
* **web:** open the project artifact stage's overflow menu ([fcd2cf2](https://github.com/abn/agent-hub/commit/fcd2cf2275ace38e58fe7404b4d6563d504c777c))
* **web:** open the project from a Storage row, not a forced Sessions tab ([d3b29a0](https://github.com/abn/agent-hub/commit/d3b29a00b331b48fcb2b107d69537fab67bc8b9d))
* **web:** open the project from Storage and reach Connect from Settings ([cea67df](https://github.com/abn/agent-hub/commit/cea67df530373ddfe1f3df88e6e651d43d2ce607))
* **web:** open the waiting item from Home, not only the queue ([993c7fd](https://github.com/abn/agent-hub/commit/993c7fdd5e1558e206fb17f6106771c16877688a))
* **web:** pluralise active agents in project header ([f3093d4](https://github.com/abn/agent-hub/commit/f3093d4a28d3be92242b4a3046843bfae23a33aa))
* **web:** point the composer field at the container radius ([ddded43](https://github.com/abn/agent-hub/commit/ddded43f40af25d2ec272c8ee97d223c246785c6))
* **web:** point the inbox swipe and pull at the shell, not a vanished screen ([f83a49c](https://github.com/abn/agent-hub/commit/f83a49c1c520850d37a40e8ee4f2b2abed6a8978))
* **web:** prevent sessions phone layout overflow and restore focus ([e5d0751](https://github.com/abn/agent-hub/commit/e5d0751a875e32166fd5c75cf79fe81617130f29))
* **web:** put the storage summary on the canvas and align the control rhythm ([aea4ba1](https://github.com/abn/agent-hub/commit/aea4ba168b7b253c31e23e96dcc7456900af0010))
* **web:** reach every search scope chip and keep wiki rows inside the pane ([905ca20](https://github.com/abn/agent-hub/commit/905ca200720487a3e2b6d106e9e27c3842df7f01))
* **web:** reach the 44px target on the comments' add control, and record the viewer's phone frame departure ([1431f43](https://github.com/abn/agent-hub/commit/1431f43bf926abe944d7b043cd343e2ea2d0d3a3))
* **web:** read a long event summary as prose, not a heading ([a0b9c8d](https://github.com/abn/agent-hub/commit/a0b9c8d6d16d19d91641f136e20a5ec451e4a84f))
* **web:** read a session's times as relative, not as a date ([9b66bcf](https://github.com/abn/agent-hub/commit/9b66bcfa63695005159d3f8afb1ce1933ae76049))
* **web:** read the home payload once per render ([e89a0da](https://github.com/abn/agent-hub/commit/e89a0daae03ca3ff715ab8e1c0de4e46ede04be4))
* **web:** read the hub's is_personal mark, and drop the stale cursor example ([2b4b5c1](https://github.com/abn/agent-hub/commit/2b4b5c140fa5ffade7d7e483a259b52cf7a555a9))
* **web:** refuse to leave the settings without rewriting history ([90a3f7e](https://github.com/abn/agent-hub/commit/90a3f7eb53e569725b8cc9978ac80efdd07c8928))
* **web:** remove desktop topbar link underlines and focus screen heading ([dc8475b](https://github.com/abn/agent-hub/commit/dc8475be5be0ca53ddc06c05d8b2a75c5c8bbbc5))
* **web:** replace viewer frame on navigation to preserve browser history ([cdfdd06](https://github.com/abn/agent-hub/commit/cdfdd065ba1aea1d8191a90e3b0936a404482a7f))
* **web:** report a secure-context requirement for protected artifacts ([2975966](https://github.com/abn/agent-hub/commit/2975966f2e3c04ce51583da525cff62664666f31))
* **web:** resolve browser URLs against the document for path-mounted hubs ([27d8d4e](https://github.com/abn/agent-hub/commit/27d8d4e5ee1681d009fb3f75d53a5b4651bbe2fa))
* **web:** restore the inbox filter, its keyboard verbs and the Home chip ([f33a120](https://github.com/abn/agent-hub/commit/f33a120d3145356ca9c1fa646a539cf3d34eec23))
* **web:** review fixes for the desktop round ([1db65a9](https://github.com/abn/agent-hub/commit/1db65a98d811ea18ec6939ba970f028c699d6c30))
* **web:** sanitize session markdown rendering and write resolved comment quotes as plain text ([31207e6](https://github.com/abn/agent-hub/commit/31207e6a444a3218ca41c4e558f9cd1e16477261))
* **web:** say what revoking a token does, and what it keeps ([c39191d](https://github.com/abn/agent-hub/commit/c39191d005e40dfd4b1dcd74e007910355ce8790))
* **web:** serve the PWA correctly behind a path-stripping proxy ([5ac86da](https://github.com/abn/agent-hub/commit/5ac86da8582bde6e1849cd25ba4e57342025f95e))
* **web:** show every recent event when today is longer than a page ([4051280](https://github.com/abn/agent-hub/commit/4051280174b1284cc20336585f210dc9b494da9c))
* **web:** size the project tabs to the design's 14px ([54f20ae](https://github.com/abn/agent-hub/commit/54f20ae5eb3bb12241fd9f610f8ddd9811371aa7))
* **web:** stop a screen left behind from painting over the current one ([70ee949](https://github.com/abn/agent-hub/commit/70ee9493448f6c5b7da9d52aebff9a27d0ca5e4d))
* **web:** stop drawing a focus ring on every screen load ([e3827d1](https://github.com/abn/agent-hub/commit/e3827d1af08b233769c727163371174fb52f4f40))
* **web:** the index is a list, and the frame stops moving between screens ([511c7b4](https://github.com/abn/agent-hub/commit/511c7b436f2964cf107fe85832a7b45157c0e62f))
* **web:** the keyboard map reaches the shell, and the panes can widen ([4f3cee3](https://github.com/abn/agent-hub/commit/4f3cee3bf474ad3c97f25cddc339894a638a46dd))
* **web:** the phone key value, the wiki tools row, the frame, and the tone ([e738cb6](https://github.com/abn/agent-hub/commit/e738cb65fa50f2bf94248397fddd1542f1ac5927))
* **web:** unify chip styling and geometry across feed and search ([1a2ac83](https://github.com/abn/agent-hub/commit/1a2ac831d31d5e43d9fb8c818e09d3463e9e8727))
* **web:** unlock a protected artifact inside the app ([175b042](https://github.com/abn/agent-hub/commit/175b0423853523ed7306b7c386db2cb7b45c5abe))
* **wiki:** drop the stray root breadcrumb and fit the section switcher ([1531ded](https://github.com/abn/agent-hub/commit/1531ded4ab4012a164c10851d18aa4f3b7dd5893))


### Performance Improvements

* **blob:** run artifact blob IO on the blocking pool ([361e75a](https://github.com/abn/agent-hub/commit/361e75ab7f66b40b10ee39d8009cf6ea325da40a))
* **storage:** weigh only the events appended since the last report ([d3386bf](https://github.com/abn/agent-hub/commit/d3386bf9782cb9974aab76a50c87f7e2362eadd7))
* **web:** draw the rail and the storage table without a request per project ([6be37c3](https://github.com/abn/agent-hub/commit/6be37c3f776a75eaebb2594626fc5341b4680d34))

## [1.0.0] - 2026-10-04

The first stable release. One binary, or one container, serves the REST API,
the installable PWA, and the MCP endpoint on a single listener over a single
engine, and no brain lives off the node. There were no tagged releases before
this one, so the section covers the project to date rather than a range between
two tags.

Two things remain intended design and are named as such wherever they appear:
background push notifications, which a closed installed app cannot raise, and
the optional embedded tailnet endpoint, which is experimental.

### Added

#### Agent surface

- MCP over streamable HTTP on the hub's own listener at `/mcp`, with a bearer
  token that resolves to one agent identity, and an embedded stdio mode behind
  `agent-hub mcp` that runs standalone against the local data directory when no
  `HUB_URL` is configured.
- Session tools: `session_start`, `session_end`, `session_list`. A session
  belongs to the agent that started it, survives compaction and the owner's
  resume, and can be adopted or forked by another agent.
- Feed tools: `feed_read` with a durable server-side read cursor per agent and
  project, and `signal_append`.
- Inbox tools: `question_post`, `answer_post`, `inbox_read`, and `inbox_wait`,
  which long-polls up to 60 seconds so an agent waits for an answer instead of
  polling.
- Artifact tools: `artifact_publish`, `artifact_update`, `artifact_get`,
  `artifact_versions`, `artifact_list`, `artifact_delete`, carrying the
  publishing actor and the total and open comment counts across every version.
- Comment tools on artifacts: `comment_post` with an anchor or quote,
  `comment_list`, `comment_resolve`, `comment_delete`.
- Brain tools over two stores through one `store` argument: `brain_get`,
  `brain_put`, `brain_list`, `brain_delete`, `brain_promote`.
- `search` over feed events, artifacts, session brains and knowledge base
  pages, scoped to a project, a session, or global.
- `whoami` and `version`.
- The agent bootstrap served at `GET /bootstrap/SKILL.md` with its own address
  filled in, and the installable `agent-hub` skill served over MCP under the
  Skills extension (`io.modelcontextprotocol/skills`): `skills/list` and
  `skills/get` carry the frontmatter and a complete manifest, and the files read
  as resources under `skill://agent-hub/`. The initialize handshake advertises
  the `resources` capability and `agent-hub tools` prints every input schema, so
  an agent wired only to MCP can discover the surface without a human handing it
  a document.
- Agent self-enrolment: `agent-hub enrol`, or `POST /api/v1/enrol` with a
  one-line explanation and a long-polled status. The operator decides from the
  inbox, and the decision is read from the approval event rather than the
  payload, so an active agent cannot forge an admission. The issued token and
  hub URL are recorded in the client config at mode `0600`.
- `session_start` reports the handoff note the previous owner of the session
  left, beside the recovery path.
- Artifact capability share links that are revocable, and version pinning.
- `base_version` on `artifact_update`, so two writers cannot silently overwrite
  each other.
- One-shot calls for a harness with no MCP client: `agent-hub call <tool>
  [json]`, `agent-hub tools`, and the `agent-hub kb <command>` shorthand over
  the project knowledge base.

#### Session brains and the project knowledge base

- A server-side, session-scoped AgentFS file per session holding that session's
  KV state, append-only audit log and POSIX-like filesystem. Agents never touch
  a file directly: the hub is the single writer per file.
- An AgentFS file per project holding the knowledge base every agent with
  project write shares, outside session life, so a prune never touches it.
- Knowledge base pages over REST and MCP with history, last writer, review
  state, comment threads, save-to-wiki, links, backlinks and a lint pass, and a
  promote route that copies a session entry into a page that cites the session
  it came from.
- The OKF v0.2 line-oriented frontmatter reader and patcher, shared by the Rust
  routes and the browser module, held to each other by a differential corpus.

#### Feed, inbox and search

- An event store and a project feed that is a first-class, time-ordered,
  addressable query, with RFC 9457 problems for every refused request.
- A global inbox for finished work and approval requests, with per-agent and
  per-project caps on open action items, a note kept on each decision,
  resolved items in Earlier, and device-local snooze.
- Engine-native full-text search across every corpus, with prefix matching,
  whole-word ranking, project and session scopes, the match count and the time
  taken reported, and a 5000-row fetch cap.
- A per-project event ceiling (`HUB_EVENTS_PER_PROJECT`, one million by default)
  that bounds every agent-surface writer, so a runaway agent cannot fill the
  node.

#### Human surface

- An installable, mobile-first PWA served as static assets from the same binary
  as the API and MCP: Home, Inbox, project feed, artifacts gallery and viewer,
  sessions with a session detail and brain tree, Storage with its prunes,
  Search, project settings, the project wiki, and Settings with agents, tokens,
  grants and access.
- The project wiki in the human surface: a home page, recent changes, reading
  and writing pages, comment threads, reviews, the editor's conflict path, and
  a save-to-wiki route.
- Project deletion with typed confirmation and a count manifest, and session
  reassignment from the session detail header.
- Accessibility held as a build gate rather than a review habit: a 12px UI text
  floor, 44px tap targets, a visible focus ring, a full keyboard path, no
  meaning carried by colour alone, reduced motion honoured, and a headless axe
  audit over every registered screen.
- An opt-in in-app notification and a server-sent freshness stream, so an open
  app refreshes its waiting badge as soon as a write lands.

#### Operations

- A distroless, non-root container image built from the `Containerfile`, and a
  compose file that mounts a named volume at `/data`, publishes 8080, keeps the
  rest of the filesystem read-only, and stops with a grace period above the
  drain.
- `agent-hub backup`, `restore`, `check` and `doctor`: an offline backup through
  the engine with a manifest of sizes and SHA-256 digests, a verified restore
  staged beside the destination and swapped by rename, the engine's integrity
  check, and a report of the schema version, the data directory's device and
  inode, free space, the write-ahead log, and the id high-water mark.
- `agent-hub health`, which GETs `/readyz` and exits non-zero when the store is
  not ready. It is what the container healthcheck runs, because the runtime
  image has no shell and no curl.
- A readiness probe that means it: the live schema version against what the
  binary supports, the identity of the data directory and the store against
  what was captured at open, a content read that catches a store which is not
  this hub's, and free space above a safety margin.
- A `SIGTERM` drain: the hub stops accepting, lets in-flight requests finish
  for a bounded window, checkpoints the store and exits 0.
- `GET /metrics` in Prometheus text, counting HTTP requests, events and tool
  calls, and a background integrity sample reported through the storage
  payload.
- Layered `config.toml` read from the system, user and `HUB_CONFIG` paths,
  overridden by environment variables, with `agent-hub config` to report the
  active settings, the search paths and the validation status.
- Real build metadata: the Version row carries the version from `Cargo.toml`
  and the short commit the build script read, so no number on screen is typed.
- An optional embedded tailnet endpoint behind the `tailnet` cargo feature,
  isolated behind one seam so the plain and embedded builds share the rest of
  the code.

### Changed

- One engine everywhere. The hub store, the per-session AgentFS files and the
  artifact storage all run on one pinned Turso engine; there is no libSQL, no
  SQLite C binding and no second engine to reconcile with.
- One process serves REST, the PWA and MCP, so the per-session write lock
  covers every writer and the prune sweeper always runs.
- The trust model is stated once: the token is the identity, a grant is access
  or no access with no read or write levels, and the admin boundary is
  privilege rather than use.
- Search ranks in the hub rather than in a separate service, and reports the
  hit count and the time it took.
- Storage is reported four ways per project and in a bar drawn to scale, and a
  prune is reversible through its undo token.
- The artifact viewer renders full markdown with themes and callouts, a
  version sheet, a group filter, and comment threads as an aside on a fine
  pointer.
- The feed is day-grouped with kind chips; the inbox groups the waiting queue
  by actor and carries each item's project by display name.
- The desktop frame does not move between screens: every pane reserves a 52px
  header and a 40px control row, in that order, whether or not it has content
  for them. On a phone the frame is one header plus one sticky tools row and
  reserves nothing, because the reader's place is their scroll position.
- The documentation is an Open Knowledge Format v0.2 bundle with a decision
  record per binding choice, a page that says whether it describes shipped
  behaviour or intended design, and a log of how the bundle evolved.
- The quality gate is one command. `make check` runs the declarative hooks, the
  one-engine check, clippy with warnings as errors, rustfmt, the OKF bundle
  validation, the static and browser PWA checks, the accessibility audit, the
  optional tailnet build, the serve-only build the image uses, and the test
  suite. CI runs the same command.

### Fixed

These were found and corrected on the pre-release line, where the version was
`0.4.1` and nothing was tagged.

#### Store and engine

- Event ids are minted in strictly increasing order and stay monotonic across a
  wall-clock step backwards.
- A prune sweep is atomic against a resume, survives a failure part way, and an
  undo is atomic against the sweep.
- Idempotency operations are namespaced and bound to their target entity, and a
  deleted comment's idempotency row is cleared rather than left to shadow a
  later write.
- A binary refuses to open a store whose schema is newer than the most it
  supports, and names both versions; a migration copies the store to
  `backups/` before it runs and keeps the three newest.
- Grant rows move with an agent whose id is renamed.
- The data directory and the store stay private to the operator.

#### Session brains

- Orphan session files are reconciled at startup, and the lock table is
  recovered from the files on disk.
- A finished brain is folded into its own file at session end and at startup.
- Brain opens are serialised and reads stay read-only under an open handle; a
  value size is capped, and the write-ahead log is removed with the brain.
- A promoted artifact blob is left to reconcile, and a missing artifact version
  names itself instead of reporting a bare failure.

#### Artifacts

- Blobs are written outside the write lock and run on the blocking pool, so a
  transfer up to the 50 MiB cap holds no async worker.
- An update that fails cleans up its promoted files, and on-disk version blobs
  are reconciled at startup.
- Protected ciphertext passes through verbatim; an unlock error, the shared
  frame loader and viewer sizing are fixed; a quote anchor is refused on a
  version the server holds only as ciphertext.

#### Client and configuration

- The one-shot CLI splits its exit codes: 1 for a tool error, 2 for usage, 69
  for an unreachable hub, 77 for a refused token and 78 for no hub named, so a
  hook can re-enrol on 77 instead of on any failure.
- MCP resources are served over the stdio proxy, and a token-gate error maps to
  its real status and is counted.
- The documented configuration keys are honoured and an unknown key fails
  startup rather than being ignored.
- The data directory is resolved without the serve credential, and the client
  token is written atomically at owner-only mode.
- `agent-hub config --check` validates, and a file holding a readable token
  warns on stderr and still works.

#### Networking and client edge

- The embedded tailnet endpoint starts as documented and rebuilds its device on
  a full session drop instead of only reconnecting the listener.
- The PWA resolves browser URLs against the document, so a hub mounted on a
  path behind a stripping proxy loads its assets.
- The app shell refreshes on a hub upgrade, follows the system theme while it is
  open, and reports the requirement for a secure context before a protected
  artifact fails to open.
