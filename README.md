# caloratto-sim

Simulador do CLP da **Carmomaq Caloratto 5 kg** falando **Modbus TCP**, para desenvolver o software de torra sem estar perto da máquina.

O simulador tem dois binários. `cmd/sim` finge ser o CLP: tem um modelo térmico com cilindro, grão, umidade, sondas com inércia, servo do gás e intertravamentos. `cmd/probe` é um cliente Modbus mínimo que lê, calcula o RoR, grava CSV e escreve comandos. Ele funciona igual contra o simulador e contra a máquina.

## Rodar

```bash
go run ./cmd/sim --auto --speed 10     # torra sozinho em loop, 10x mais rápido
go run ./cmd/probe --time-scale 10     # em outro terminal
```

O `--auto` liga um "operador" que pré-aquece, carrega 5 kg a 200 °C, ajusta gás e ar por faixa de temperatura, descarrega a 210 °C, resfria e repete. Sem `--auto`, quem comanda é você (ou o seu app).

### Painel do simulador (HTTP :8080)

| Rota | O que faz |
|---|---|
| `/state` | Estado completo em JSON, incluindo a **verdade interna** (temperatura real do grão, umidade), útil para validar a detecção de eventos do app |
| `/charge?kg=5&green=25` | Carrega café (na máquina real é a alavanca da moega) |
| `/drop` | Descarrega |
| `/preheat?temp=200` | Pula o aquecimento |
| `/fault?name=flame&on=1` | Simula falta de chama (o gás fecha) |
| `/fault?name=emergency&on=1` | Simula o botão de emergência (desliga tudo) |
| `/speed?x=30` | Muda a aceleração do tempo |
| `/reset` | Volta a máquina fria |

### Escrever comandos

```bash
go run ./cmd/probe set drum_on=1 fan_on=1 burner=1 gas_cmd=60
```

A escrita é **bloqueada** quando o alvo não é localhost. Para a máquina real, é preciso passar `--allow-write` explicitamente, e isso deve ser feito **só com alguém ao lado do torrador**.

## Contra a máquina real

```bash
go run ./cmd/probe --url tcp://192.168.5.11:502 --csv torra.csv
```

O PC precisa estar na rede `192.168.5.x`, como mostra a pág. 23 do manual, ou ligado via Tailscale.

## ⚠️ O mapa de registradores é provisório

Os endereços em `configs/regmap.json` foram **inventados**. Quando o `.aset` da Carmomaq chegar, troque `kind`, `addr`, `scale` e `signed` de cada ponto. Os `name` são fixos, porque é por eles que o simulador e o app se encontram. Não precisa mexer em código.

## O que o modelo cobre (e o que não cobre)

Pontos calibrados para parecer uma torra real:
- **Turning point** em ~80 °C por volta de 1:15.
- **RoR** com pico de ~25 °C/min, caindo para ~8 no fim.
- **1º crack** na faixa de 196 °C por volta de 8–9 min, com descarga a 210 °C em ~10:45.
- **Dip do ET** na carga.
- Conversão de % para **rpm** e **m³/h** pela tabela da pág. 16 do manual.

Intertravamentos simulados. Eles são suposições razoáveis e precisam ser confirmados na máquina:
- O queimador só acende com cilindro **e** ventilador ligados.
- Falta de chama fecha o gás.
- O botão de emergência desliga tudo e recusa religar o queimador.
- O limite de segurança de 280 °C corta o queimador, como na tela de Opções do manual.

O que não é exato:
- A física é simplificada (modelo de parâmetros concentrados).
- A perda de peso fica em ~9%; o real é 12–15%, porque o modelo não simula perda orgânica.
- Os números servem para desenvolver a UI e os algoritmos, não para prever torra.

`go test ./...` verifica se a curva continua plausível (faixa de TP e duração) e se os intertravamentos funcionam.
