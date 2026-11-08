# Teste de ponta a ponta do atlasdisco-cli (Windows). Uso: powershell -File test\e2e.ps1 [-Exe caminho]
param([string] $Exe = (Join-Path $PSScriptRoot '..\target\release\atlasdisco-cli.exe'))
$ErrorActionPreference = 'Continue'  # o CLI escreve progresso em stderr
$root = Join-Path $env:TEMP ('atlasdisco-e2e-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$out = Join-Path $root '_saida'
$d = Join-Path $root 'dados'
$fails = 0
function Check([string] $name, [bool] $ok, [string] $detail = '') {
  '{0,-52} {1}' -f $name, $(if ($ok) { 'OK' } else { "FALHOU $detail" })
  if (-not $ok) { $script:fails++ }
}
function Bytes([string] $path, [int] $n, [int] $seed) {
  $b = New-Object byte[] $n; (New-Object Random $seed).NextBytes($b); [IO.File]::WriteAllBytes($path, $b)
}
function Run { $o = & $Exe @args 2>&1 | Out-String; return $o }

New-Item -ItemType Directory -Force "$d\fotos\2024", "$d\videos", "$d\docs", $out | Out-Null
Bytes "$d\fotos\2024\a.jpg" 300000 1
Bytes "$d\fotos\2024\copia-de-a.jpg" 300000 1        # duplicado de a.jpg
Bytes "$d\videos\filme.mp4" 2000000 2
Bytes "$d\videos\filme-quase.mp4" 2000000 3          # mesmo tamanho, conteúdo diferente
Set-Content "$d\docs\nota.txt" 'relatorio com a palavra CHAVE no meio' -Encoding UTF8
Set-Content "$d\docs\velho.tmp" 'temporario' -Encoding UTF8
(Get-Item "$d\docs\velho.tmp").LastWriteTime = (Get-Date).AddYears(-3)
cmd /c mklink /H "$d\docs\link-para-filme.mp4" "$d\videos\filme.mp4" | Out-Null
cmd /c mklink /J "$d\volta" "$d" | Out-Null                    # junção que aponta para cima (ciclo)
$long = "\\?\$d\" + (('pasta_com_nome_comprido_' * 1) + '1\') * 12
New-Item -ItemType Directory -Force $long | Out-Null
[IO.File]::WriteAllText($long + 'fundo.txt', 'x' * 1000)
Bytes "$d\docs\compactavel.bin" 0 0
[IO.File]::WriteAllText("$d\docs\compactavel.bin", ('A' * 400000))
compact /c /q "$d\docs\compactavel.bin" | Out-Null

# Conta de referência, independente do AtlasDisco (sem seguir a junção).
$ref = 0; $refFiles = 0
$stack = New-Object System.Collections.Stack; $stack.Push((New-Object IO.DirectoryInfo ("\\?\" + $d)))
while ($stack.Count) {
  $di = $stack.Pop()
  foreach ($f in $di.GetFiles()) { $ref += $f.Length; $refFiles++ }
  foreach ($s in $di.GetDirectories()) { if (-not ($s.Attributes -band [IO.FileAttributes]::ReparsePoint)) { $stack.Push($s) } }
}

$o = Run varrer $d --profundidade 20 --arquivos -e "$out\r.csv" -e "$out\r.xlsx" -e "$out\r.html" -e "$out\r.pdf" -e "$out\r.json" -e "$out\r.sqlite" -e "$out\r.txt" --salvar "$out\s1.atlas"
$csv = Import-Csv "$out\r.csv" -Delimiter ';' -Encoding UTF8
$rootRow = $csv | Where-Object { $_.'Nível' -eq '0' }
Check 'total igual à contagem independente' ([int64]$rootRow.Tamanho -eq $ref) "atlasdisco=$($rootRow.Tamanho) ref=$ref"
Check 'número de arquivos igual' ([int64]$rootRow.Arquivos -eq $refFiles) "atlasdisco=$($rootRow.Arquivos) ref=$refFiles"
Check 'junção não seguida (sem ciclo)' (-not ($csv | Where-Object { $_.Caminho -like '*\volta\*' }))
Check 'caminho com mais de 260 caracteres' ([bool]($csv | Where-Object { $_.Caminho -like '*fundo.txt' -and $_.Caminho.Length -gt 260 }))
$comp = $csv | Where-Object { $_.Caminho -like '*compactavel.bin' }
Check 'compressão NTFS: alocado < tamanho' ([int64]$comp.Alocado -lt [int64]$comp.Tamanho) "tam=$($comp.Tamanho) aloc=$($comp.Alocado)"
foreach ($e in 'csv', 'xlsx', 'html', 'pdf', 'json', 'sqlite', 'txt') { Check "exportação .$e" ((Get-Item "$out\r.$e").Length -gt 200) }
Check 'PDF válido' ([Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes("$out\r.pdf"), 0, 8) -eq '%PDF-1.4')
Check 'XLSX é um zip' ([IO.File]::ReadAllBytes("$out\r.xlsx")[0] -eq 0x50)

# Hardlink contado uma vez: o alocado cai, o tamanho lógico continua.
& $Exe varrer $d --hardlinks --profundidade 0 -e "$out\h.csv" 2>&1 | Out-Null
$h = (Import-Csv "$out\h.csv" -Delimiter ';' -Encoding UTF8)[0]
Check 'hardlink: alocado conta o filme uma vez' (([int64]$rootRow.Alocado - [int64]$h.Alocado) -ge 2000000) "sem=$($rootRow.Alocado) com=$($h.Alocado)"

$o = Run duplicados $d --min 1KB -e "$out\dup.csv"
$dup = Import-Csv "$out\dup.csv" -Delimiter ';' -Encoding UTF8
$g1 = $dup | Where-Object { $_.Caminho -like '*a.jpg' }
Check 'duplicados: a.jpg e cópia no mesmo grupo' ($g1.Count -eq 2 -and ($g1.Grupo | Select-Object -Unique).Count -eq 1)
Check 'duplicados: filme-quase.mp4 não é duplicado' (-not ($dup | Where-Object { $_.Caminho -like '*filme-quase.mp4' }))

$o = Run buscar $d --conteudo chave
Check 'busca por conteúdo' ($o -match 'nota\.txt' -and $o -match '1 resultados')
$o = Run buscar $d --nome '*.tmp' --antigo-dias 700
Check 'busca por nome + idade' ($o -match 'velho\.tmp' -and $o -match '1 resultados')
$o = Run buscar $d --caminho-min 261 --tipo ambos
Check 'busca de caminhos longos' ($o -match 'fundo\.txt')

Bytes "$d\videos\novo.mkv" 5000000 9
Remove-Item "$d\fotos" -Recurse -Force
$o = Run comparar "$out\s1.atlas" $d --profundidade 3 --arquivos -e "$out\cmp.csv"
$cmp = Import-Csv "$out\cmp.csv" -Delimiter ';' -Encoding UTF8
Check 'comparação: novo arquivo' ([bool]($cmp | Where-Object { $_.Caminho -eq 'videos\novo.mkv' -and $_.'Situação' -eq 'novo' }))
Check 'comparação: pasta removida' ([bool]($cmp | Where-Object { $_.Caminho -eq 'fotos' -and $_.'Situação' -eq 'removido' }))
Check 'comparação: variação de videos' ([bool]($cmp | Where-Object { $_.Caminho -eq 'videos' -and [int64]$_.'Diferença (bytes)' -eq 5000000 }))

$o = Run varrer (Join-Path $root 'nao-existe')
Check 'pasta inexistente dá erro' ($LASTEXITCODE -ne 0 -and $o -match 'erro')

cmd /c rmdir "$d\volta" | Out-Null
Remove-Item -LiteralPath ("\\?\" + $root) -Recurse -Force
"falhas: $fails"
exit $fails
