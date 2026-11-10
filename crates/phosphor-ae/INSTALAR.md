# Como instalar o Phosphor no After Effects

Phosphor é um efeito de After Effects que deixa qualquer vídeo com cara de TV analógica: câmera
digital, sinal NTSC/VHS, glitches de TV e videocassete, tela de tubo (CRT) e o túnel de
"câmera filmando a própria TV". É uma versão para After Effects do app **NTSCRT**, de Finn McKenty.

> **Feito com IA ("vibecoded").** O código foi escrito por uma IA (Claude, pelo Claude Code) para
> um autor que não é programador, e testado no After Effects. É gratuito e de código aberto.
> Os créditos de todo mundo que fez a parte difícil estão em `CREDITS.md`.

---

## Do que você precisa

- Windows 10 ou 11, 64 bits.
- Adobe After Effects (testado na versão 2026; versões recentes devem funcionar).
- Uma placa de vídeo com Direct3D 11 (praticamente qualquer uma dos últimos 10 anos). Sem ela o
  efeito ainda funciona, só que mais devagar.

## Passo a passo

### 1. Feche o After Effects

Com ele aberto, o Windows não deixa copiar o plugin.

### 2. Baixe e descompacte o Phosphor

Baixe o `Phosphor-...-windows.zip` da página de downloads:
https://github.com/Alev0000/phosphor/releases/latest

Clique com o botão direito no zip › **Extrair Tudo…**. Sai uma pasta `Phosphor`; dentro dela devem estar:

```
Phosphor\
├─ Phosphor.aex      o plugin
├─ shaders\          os shaders de tela de tubo (não apague, o efeito precisa deles)
├─ LICENSES\         as licenças
└─ CREDITS.md        os créditos
```

### 3. Abra a pasta de plugins da Adobe

No Explorador de Arquivos, cole este endereço na barra de cima e aperte Enter:

```
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore
```

### 4. Copie a pasta `Phosphor` inteira para dentro dela

Arraste a pasta `Phosphor` (a pasta, não só o `Phosphor.aex`) para dentro de `MediaCore`. O Windows
vai pedir permissão de administrador: clique em **Continuar**.

O resultado tem que ser:

```
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor\Phosphor.aex
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor\shaders\...
```

### 5. Abra o After Effects e aplique o efeito

1. Selecione uma camada (vídeo, imagem, o que for).
2. Menu **Effect › Stylize › Phosphor**, ou procure "Phosphor" no painel **Effects & Presets**.
3. No menu **Look**, escolha um dos 27 looks prontos do NTSCRT. Pronto.

---

## Primeiros passos

| Controle | O que faz |
|---|---|
| **Look** | 27 looks prontos. Os animados viram keyframes do After Effects. |
| **Load / Save Look File…** | Abre e salva looks no formato do NTSCRT (funcionam nos dois). |
| **Digital Video** | Câmera e codec digital: Webcam, Video call, MiniDV, Early web video, Datamosh. |
| **NTSC (TV)** | O sinal analógico (ntsc-rs): ruído, cor vazando, VHS. |
| **Downscale** | Baixa a resolução (SNES, VGA ou largura própria). |
| **Glitch** | Defeitos de recepção e de fita: sinal fraco, imagem rolando, fantasma, tracking. |
| **CRT** | A tela de tubo: 7 shaders do RetroArch. |
| **Screen Loop** | O túnel infinito: uma câmera filmando a própria TV. Arraste o **Vanishing point** na tela da comp. |

Dicas:

- O Screen Loop é calculado em ordem, frame a frame. Ao pular para o meio da timeline, ele leva
  alguns segundos para montar o túnel; tocando para frente, é rápido.
- **Camera renders on** (no Screen Loop): deixe em **GPU (recommended)**. **CPU (exact)** é a
  referência, mais lenta.

---

## Problemas comuns

**O efeito não aparece no menu.**
Confira se o caminho está exatamente como no passo 4 (a pasta `Phosphor` dentro de `MediaCore`) e
reinicie o After Effects. Procure "Phosphor" no painel Effects & Presets.

**O CRT não muda nada / aparece erro de shader.**
A pasta `shaders` precisa estar ao lado do `Phosphor.aex`. Copie a pasta `Phosphor` inteira de novo.

**"Acesso negado" ao copiar.**
O After Effects (ou o Premiere / Media Encoder) ainda está aberto. Feche todos e tente de novo.

**Algo deu errado e quero mandar para alguém olhar.**
O Phosphor grava um registro em `%TEMP%\phosphor-ae.log` (cole esse endereço no Explorador).

## Desinstalar

Feche o After Effects e apague a pasta
`C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor`.

## Atualizar

Igual à instalação: feche o After Effects e copie a pasta `Phosphor` nova por cima da antiga,
aceitando substituir os arquivos.
