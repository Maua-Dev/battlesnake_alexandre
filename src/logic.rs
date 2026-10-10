// Bem-vindo ao
// __________         __    __  .__                               __
// \______   \_____ _/  |__/  |_|  |   ____   ______ ____ _____  |  | __ ____
//  |    |  _/\__  \   __\   __\  | _/ __ \ /  ___//    \__  \ |  |/ // __ \
//  |    |   \ / __ \|  |  |  | |  |_\  ___/ \___ \|   |  \/ __ \|    <\  ___/
//  |________/(______/__|  |__| |____/\_____>______>___|__(______/__|__\_____>
//
// ESTE É O ARQUIVO QUE VOCÊ VAI EDITAR. Todo o resto do projeto existe
// só para levar o estado do jogo até as quatro funções abaixo.
//
// Para começar, já deixamos pronta a lógica que impede a sua cobra de andar
// para trás (ela morreria na hora). Os TODOs marcam os próximos passos.
// Documentação: https://docs.battlesnake.com

use crate::models::Battlesnake;
use crate::models::GameState;
use rand::seq::IndexedRandom;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::info;

/// GET / — chamado quando você cadastra a cobra no site e a cada partida.
/// Controla a aparência dela. Opções de cabeça, cauda e cor:
/// https://docs.battlesnake.com/guides/customizations
pub fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "xandealee",          // TODO: coloque aqui o SEU usuário do Battlesnake
        "color": "#008a25",    // TODO: escolha a cor da sua cobra
        "head": "tiger-king",  // TODO: escolha a cabeça
        "tail": "hook",        // TODO: escolha a cauda
        "version": "1.0.0"
    })
}

/// POST /start — chamado uma vez, quando a partida começa.
/// Bom lugar para preparar qualquer estado inicial.
pub fn start(state: &GameState) {
    info!("JOGO COMEÇOU (partida {})", state.game.id);
}

/// POST /end — chamado uma vez, quando a partida termina.
pub fn end(state: &GameState) {
    info!("FIM DE JOGO após {} turnos", state.turn);
}

// ---------------------------------------------------------------------------
// Funções auxiliares da lógica avançada (espaço, adversárias, comida)
// ---------------------------------------------------------------------------

/// (nome, dx, dy) — no Battlesnake "up" aumenta o y.
const DIRECTIONS: [(&str, i32, i32); 4] = [
    ("up", 0, 1),
    ("down", 0, -1),
    ("left", -1, 0),
    ("right", 1, 0),
];

fn in_bounds(x: i32, y: i32, width: i32, height: i32) -> bool {
    x >= 0 && y >= 0 && x < width && y < height
}

/// Tabuleiro de casas bloqueadas (corpos de TODAS as cobras, inclusive a minha).
/// O rabo não conta como bloqueado: ele sai do lugar no próximo turno,
/// a não ser que a cobra tenha acabado de comer (vida == 100).
fn build_blocked(state: &GameState) -> Vec<Vec<bool>> {
    let width = state.board.width;
    let height = state.board.height;
    let mut blocked = vec![vec![false; height as usize]; width as usize];

    for snake in state.board.snakes.iter().chain(std::iter::once(&state.you)) {
        let len = snake.body.len();
        for (i, seg) in snake.body.iter().enumerate() {
            let is_tail = i == len - 1;
            if is_tail && len > 1 && snake.health != 100 {
                continue;
            }
            if in_bounds(seg.x, seg.y, width, height) {
                blocked[seg.x as usize][seg.y as usize] = true;
            }
        }
    }
    blocked
}

/// Casas ao lado da cabeça de cobras adversárias MAIORES OU IGUAIS a mim:
/// se eu for para lá, posso perder (ou empatar) um head-to-head.
fn build_danger(state: &GameState) -> Vec<Vec<bool>> {
    let width = state.board.width;
    let height = state.board.height;
    let my_len = state.you.body.len();
    let mut danger = vec![vec![false; height as usize]; width as usize];

    for snake in &state.board.snakes {
        if snake.id == state.you.id || snake.body.len() < my_len {
            continue;
        }
        let head = &snake.body[0];
        for (_, dx, dy) in DIRECTIONS {
            let (nx, ny) = (head.x + dx, head.y + dy);
            if in_bounds(nx, ny, width, height) {
                danger[nx as usize][ny as usize] = true;
            }
        }
    }
    danger
}

/// Flood fill (BFS): quantas casas livres consigo alcançar a partir de `start`.
/// Para quando chega em `limit`, porque saber "tem espaço de sobra" já basta.
fn flood_fill(
    start: (i32, i32),
    blocked: &[Vec<bool>],
    width: i32,
    height: i32,
    limit: usize,
) -> usize {
    let mut visited = vec![vec![false; height as usize]; width as usize];
    let mut queue = VecDeque::new();
    visited[start.0 as usize][start.1 as usize] = true;
    queue.push_back(start);
    let mut count = 0;

    while let Some((x, y)) = queue.pop_front() {
        count += 1;
        if count >= limit {
            return count;
        }
        for (_, dx, dy) in DIRECTIONS {
            let (nx, ny) = (x + dx, y + dy);
            if in_bounds(nx, ny, width, height)
                && !visited[nx as usize][ny as usize]
                && !blocked[nx as usize][ny as usize]
            {
                visited[nx as usize][ny as usize] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Busca com lookahead: minimax com movimentos simultâneos + alpha-beta,
// aprofundamento iterativo e avaliação por território (Voronoi).
// ---------------------------------------------------------------------------

const NO_CELL: usize = usize::MAX;
const HAZARD_DAMAGE: i32 = 14;
const INF: i32 = 1_000_000;
const WIN: i32 = 100_000;
const LOSS: i32 = -100_000;
const DRAW: i32 = -30_000;
const UNREACHABLE: i32 = 10_000;
const MAX_DEPTH: i32 = 24;
/// Quantos inimigos (os mais próximos) são testados em todas as respostas.
const FULL_BRANCH_OPPONENTS: usize = 2;
/// Fração do timeout (em %) usada pela busca. Sobra o resto para rede/Lambda.
const BUDGET_PERCENT: i64 = 45;

/// Geometria do tabuleiro: célula = y * largura + x, vizinhos pré-calculados.
struct Geo {
    width: i32,
    height: i32,
    cells: usize,
    nbr: Vec<[usize; 4]>,
    hazard: Vec<i32>,
}

impl Geo {
    fn new(state: &GameState) -> Geo {
        let width = state.board.width;
        let height = state.board.height;
        let cells = (width * height) as usize;
        let mut nbr = vec![[NO_CELL; 4]; cells];
        for y in 0..height {
            for x in 0..width {
                for (d, &(_, dx, dy)) in DIRECTIONS.iter().enumerate() {
                    let (nx, ny) = (x + dx, y + dy);
                    if in_bounds(nx, ny, width, height) {
                        nbr[(y * width + x) as usize][d] = (ny * width + nx) as usize;
                    }
                }
            }
        }
        let mut hazard = vec![0; cells];
        for c in &state.board.hazards {
            if in_bounds(c.x, c.y, width, height) {
                hazard[(c.y * width + c.x) as usize] += 1;
            }
        }
        Geo { width, height, cells, nbr, hazard }
    }

    fn cell(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    fn manhattan(&self, a: usize, b: usize) -> i32 {
        let w = self.width as usize;
        ((a % w) as i32 - (b % w) as i32).abs() + ((a / w) as i32 - (b / w) as i32).abs()
    }

    /// rel[casa] = em quantos turnos a casa fica livre (0 = livre agora,
    /// 1 = rabo que sai neste turno).
    fn release(&self, s: &Sim, rel: &mut [i32]) {
        rel.fill(0);
        for sn in &s.snakes {
            if !sn.alive {
                continue;
            }
            let len = sn.body.len();
            for (k, &c) in sn.body.iter().enumerate() {
                let r = (len - k) as i32;
                if r > rel[c] {
                    rel[c] = r;
                }
            }
        }
    }
}

/// Cobra do modelo interno. body[0] é a cabeça.
#[derive(Clone)]
struct Snk {
    body: Vec<usize>,
    health: i32,
    alive: bool,
}

/// Estado simulado. snakes[0] é sempre a minha cobra.
#[derive(Clone)]
struct Sim {
    food: Vec<bool>,
    snakes: Vec<Snk>,
}

fn to_snk(geo: &Geo, snake: &Battlesnake) -> Option<Snk> {
    let head = snake.body.first()?;
    if !in_bounds(head.x, head.y, geo.width, geo.height) {
        return None;
    }
    let body: Vec<usize> = snake
        .body
        .iter()
        .filter(|c| in_bounds(c.x, c.y, geo.width, geo.height))
        .map(|c| geo.cell(c.x, c.y))
        .collect();
    Some(Snk { body, health: snake.health as i32, alive: true })
}

fn build_sim(state: &GameState, geo: &Geo) -> Option<Sim> {
    let mut snakes = vec![to_snk(geo, &state.you)?];
    for other in &state.board.snakes {
        if other.id == state.you.id {
            continue;
        }
        if let Some(snk) = to_snk(geo, other) {
            snakes.push(snk);
        }
    }
    let mut food = vec![false; geo.cells];
    for f in &state.board.food {
        if in_bounds(f.x, f.y, geo.width, geo.height) {
            food[geo.cell(f.x, f.y)] = true;
        }
    }
    Some(Sim { food, snakes })
}

/// BFS em turnos a partir de uma cabeça, respeitando rabos que vão sair.
fn bfs(geo: &Geo, rel: &[i32], head: usize, dist: &mut [i32], queue: &mut Vec<usize>) {
    dist.fill(UNREACHABLE);
    queue.clear();
    dist[head] = 0;
    queue.push(head);
    let mut qh = 0;
    while qh < queue.len() {
        let cell = queue[qh];
        qh += 1;
        let nd = dist[cell] + 1;
        for &nn in &geo.nbr[cell] {
            if nn == NO_CELL || dist[nn] != UNREACHABLE || rel[nn] > nd {
                continue;
            }
            dist[nn] = nd;
            queue.push(nn);
        }
    }
}

fn promote(moves: &mut [usize], preferred: usize) {
    if let Some(i) = moves.iter().position(|&m| m == preferred) {
        moves[..=i].rotate_right(1);
    }
}

struct Search<'a> {
    geo: &'a Geo,
    deadline: Instant,
    had_opponents: bool,
    timed_out: bool,
    nodes: u64,
    completed_depth: i32,
    root_order: Vec<usize>,
    root_best: Option<usize>,
    // buffers reutilizáveis da avaliação
    rel_eval: Vec<i32>,
    dist: Vec<Vec<i32>>,
    queue: Vec<usize>,
}

impl<'a> Search<'a> {
    fn new(geo: &'a Geo, deadline: Instant, snakes: usize) -> Search<'a> {
        Search {
            geo,
            deadline,
            had_opponents: snakes > 1,
            timed_out: false,
            nodes: 0,
            completed_depth: 0,
            root_order: Vec::new(),
            root_best: None,
            rel_eval: vec![0; geo.cells],
            dist: vec![vec![UNREACHABLE; geo.cells]; snakes],
            queue: Vec::with_capacity(geo.cells),
        }
    }

    fn expired(&mut self) -> bool {
        if !self.timed_out && Instant::now() >= self.deadline {
            self.timed_out = true;
        }
        self.timed_out
    }

    /// Aprofundamento iterativo: refaz a busca com profundidade 1, 2, 3...
    /// e guarda a melhor jogada da última profundidade COMPLETA.
    fn run(&mut self, root: &Sim) -> Option<usize> {
        let geo = self.geo;
        let mut rel = vec![0; geo.cells];
        geo.release(root, &mut rel);
        let legal = self.legal_moves(root, 0, &rel);
        if legal.len() == 1 {
            return Some(legal[0]);
        }
        self.root_order = self.order_moves(root, 0, legal, &rel);

        let mut best = None;
        for depth in 1..=MAX_DEPTH {
            self.root_best = None;
            let v = self.value(root, depth, -INF, INF, true);
            if self.timed_out {
                break;
            }
            self.completed_depth = depth;
            if let Some(m) = self.root_best {
                best = Some(m);
                promote(&mut self.root_order, m);
            }
            if v > WIN / 2 {
                break; // vitória forçada encontrada
            }
        }
        best.or_else(|| self.root_order.first().copied())
    }

    /// valor(nó) = max sobre meus movimentos de min sobre as respostas
    /// conjuntas dos inimigos (todos se movem ao mesmo tempo).
    fn value(&mut self, s: &Sim, depth: i32, mut alpha: i32, beta: i32, is_root: bool) -> i32 {
        if self.expired() {
            return 0;
        }
        self.nodes += 1;

        let n = s.snakes.len();
        let alive_opps = (1..n).filter(|&i| s.snakes[i].alive).count();

        if !s.snakes[0].alive {
            return if self.had_opponents && alive_opps == 0 { DRAW } else { LOSS - depth };
        }
        if self.had_opponents && alive_opps == 0 {
            return WIN + depth;
        }
        if depth == 0 {
            return self.eval(s);
        }

        let geo = self.geo;
        let mut rel = vec![0; geo.cells];
        geo.release(s, &mut rel);

        let my_moves = if is_root {
            self.root_order.clone()
        } else {
            let legal = self.legal_moves(s, 0, &rel);
            self.order_moves(s, 0, legal, &rel)
        };

        // Inimigos mais próximos da minha cabeça primeiro.
        let my_head = s.snakes[0].body[0];
        let mut opps: Vec<usize> = (1..n).filter(|&i| s.snakes[i].alive).collect();
        opps.sort_by_key(|&i| geo.manhattan(s.snakes[i].body[0], my_head));
        let mut k = opps.len().min(FULL_BRANCH_OPPONENTS);
        while k < opps.len() && geo.manhattan(s.snakes[opps[k]].body[0], my_head) <= 2 {
            k += 1;
        }

        // Os k primeiros são testados em todas as respostas; os demais
        // fazem um único movimento "provável".
        let mut mv = vec![NO_CELL; n];
        let mut full_moves: Vec<Vec<usize>> = Vec::new();
        for (j, &i) in opps.iter().enumerate() {
            let legal = self.legal_moves(s, i, &rel);
            let ordered = self.order_moves(s, i, legal, &rel);
            if j < k {
                full_moves.push(ordered);
            } else {
                mv[i] = ordered[0];
            }
        }

        let mut best = -INF;
        for &m in &my_moves {
            mv[0] = m;
            let mut pos = vec![0usize; k];
            let mut cur = INF;

            loop {
                if self.expired() {
                    return 0;
                }
                for j in 0..k {
                    mv[opps[j]] = full_moves[j][pos[j]];
                }

                let child = self.step(s, &mv);
                let v = self.value(&child, depth - 1, alpha, beta.min(cur), false);
                if self.timed_out {
                    return 0;
                }
                if v < cur {
                    cur = v;
                }
                if cur <= alpha {
                    break; // poda: o adversário já me garante menos que o que tenho
                }

                // Próxima combinação de respostas (odômetro).
                let mut advanced = false;
                for j in (0..k).rev() {
                    pos[j] += 1;
                    if pos[j] < full_moves[j].len() {
                        advanced = true;
                        break;
                    }
                    pos[j] = 0;
                }
                if !advanced {
                    break;
                }
            }

            if cur > best {
                best = cur;
                if is_root {
                    self.root_best = Some(m);
                }
            }
            if best > alpha {
                alpha = best;
            }
            if alpha >= beta {
                break;
            }
        }
        best
    }

    /// Movimentos que não matam na hora (parede/corpo). O rabo comum sai do
    /// lugar; o rabo "empilhado" (cobra que acabou de comer) não.
    fn legal_moves(&self, s: &Sim, i: usize, rel: &[i32]) -> Vec<usize> {
        let body = &s.snakes[i].body;
        let head = body[0];
        let mut result = Vec::with_capacity(4);
        let mut any_dir = None;
        for d in 0..4 {
            let next = self.geo.nbr[head][d];
            if next == NO_CELL {
                continue;
            }
            any_dir.get_or_insert(d);
            if body.len() > 1 && next == body[1] {
                continue;
            }
            if rel[next] > 1 {
                continue;
            }
            result.push(d);
        }
        if result.is_empty() {
            result.push(any_dir.unwrap_or(0)); // todas matam: a cobra morre na simulação
        }
        result
    }

    /// Nota rápida de um movimento, só para ORDENAR a busca (melhora a poda).
    fn move_score(&self, s: &Sim, i: usize, d: usize, rel: &[i32]) -> i32 {
        let sn = &s.snakes[i];
        let next = self.geo.nbr[sn.body[0]][d];
        if next == NO_CELL || rel[next] > 1 {
            return LOSS;
        }
        let hp = if s.food[next] {
            100
        } else {
            sn.health - 1 - self.geo.hazard[next] * HAZARD_DAMAGE
        };
        if hp <= 0 {
            return LOSS + hp;
        }
        let mut score = 0;
        // risco de head-to-head contra cobras maiores ou iguais
        for (j, o) in s.snakes.iter().enumerate() {
            if j != i && o.alive && o.body.len() >= sn.body.len()
                && self.geo.nbr[o.body[0]].contains(&next)
            {
                score -= 20_000;
            }
        }
        let exits = self.geo.nbr[next]
            .iter()
            .filter(|&&a| a != NO_CELL && a != sn.body[0] && rel[a] <= 2)
            .count() as i32;
        score += 70 * exits + hp - 8 * self.geo.hazard[next] * HAZARD_DAMAGE;
        let nearest = (0..self.geo.cells)
            .filter(|&c| s.food[c])
            .map(|c| self.geo.manhattan(next, c))
            .min();
        if let Some(dist) = nearest {
            score -= (if sn.health < 40 { 12 } else { 2 }) * dist;
        }
        if s.food[next] {
            score += if sn.health < 40 { 600 } else { 90 };
        }
        score
    }

    fn order_moves(&self, s: &Sim, i: usize, moves: Vec<usize>, rel: &[i32]) -> Vec<usize> {
        let mut scored: Vec<(i32, usize)> = moves
            .into_iter()
            .map(|d| (self.move_score(s, i, d, rel), d))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, d)| d).collect()
    }

    /// Simula UM turno com as regras padrão: mover, perder vida (e hazard),
    /// comer/crescer e eliminar (parede, corpo, head-to-head).
    fn step(&self, s: &Sim, mv: &[usize]) -> Sim {
        let geo = self.geo;
        let n = s.snakes.len();
        let mut next = s.clone();
        let mut ate = vec![false; n];

        for i in 0..n {
            if !next.snakes[i].alive {
                continue;
            }
            let head = next.snakes[i].body[0];
            let nc = if mv[i] < 4 { geo.nbr[head][mv[i]] } else { NO_CELL };
            let sn = &mut next.snakes[i];
            if nc == NO_CELL {
                sn.alive = false; // saiu do tabuleiro
                continue;
            }
            sn.body.rotate_right(1);
            sn.body[0] = nc;
            if s.food[nc] {
                ate[i] = true;
                sn.health = 100;
            } else {
                sn.health -= 1 + geo.hazard[nc] * HAZARD_DAMAGE;
                if sn.health <= 0 {
                    sn.alive = false;
                }
            }
        }

        // Quem comeu cresce (rabo duplicado) e a comida some.
        for i in 0..n {
            if ate[i] {
                let sn = &mut next.snakes[i];
                let last = *sn.body.last().unwrap();
                sn.body.push(last);
                next.food[sn.body[0]] = false;
            }
        }

        // Colisões, todas calculadas sobre o mesmo estado.
        let contender: Vec<bool> = next.snakes.iter().map(|sn| sn.alive).collect();
        let mut kill = vec![false; n];
        for i in 0..n {
            if !contender[i] {
                continue;
            }
            let head = next.snakes[i].body[0];
            for j in 0..n {
                if !contender[j] {
                    continue;
                }
                let other = &next.snakes[j].body;
                if other[1..].contains(&head)
                    || (j != i && other[0] == head && other.len() >= next.snakes[i].body.len())
                {
                    kill[i] = true;
                    break;
                }
            }
        }
        for i in 0..n {
            if kill[i] {
                next.snakes[i].alive = false;
            }
        }
        next
    }

    /// Avaliação de uma posição, do meu ponto de vista.
    fn eval(&mut self, s: &Sim) -> i32 {
        let geo = self.geo;
        let n = s.snakes.len();
        geo.release(s, &mut self.rel_eval);

        for i in 0..n {
            if s.snakes[i].alive {
                bfs(geo, &self.rel_eval, s.snakes[i].body[0], &mut self.dist[i], &mut self.queue);
            }
        }

        // Território (Voronoi): cada casa é de quem chega primeiro.
        // Empate: vence a cobra estritamente maior; senão, ninguém.
        let mut territory = vec![0i32; n];
        for c in 0..geo.cells {
            let best_d = (0..n)
                .filter(|&i| s.snakes[i].alive)
                .map(|i| self.dist[i][c])
                .min()
                .unwrap_or(UNREACHABLE);
            if best_d >= UNREACHABLE {
                continue;
            }
            let mut winner = 0;
            let mut winner_len = 0;
            let mut unique = true;
            for i in 0..n {
                if !s.snakes[i].alive || self.dist[i][c] != best_d {
                    continue;
                }
                let len = s.snakes[i].body.len();
                if len > winner_len {
                    winner_len = len;
                    winner = i;
                    unique = true;
                } else if len == winner_len {
                    unique = false;
                }
            }
            if unique {
                territory[winner] += 1;
            }
        }

        let me = &s.snakes[0];
        let my_len = me.body.len() as i32;
        let mut opp_max_cells = 0;
        let mut opp_max_len = 0;
        for i in 1..n {
            if s.snakes[i].alive {
                opp_max_cells = opp_max_cells.max(territory[i]);
                opp_max_len = opp_max_len.max(s.snakes[i].body.len() as i32);
            }
        }

        let self_space = self.dist[0].iter().filter(|&&d| d < UNREACHABLE).count() as i32;
        let nearest_food = (0..geo.cells)
            .filter(|&c| s.food[c] && self.dist[0][c] < UNREACHABLE)
            .map(|c| self.dist[0][c])
            .min();
        let has_food = s.food.iter().any(|&f| f);

        let mut score = 0;

        // Território: o que mais separa cobras fortes de fracas.
        score += 14 * territory[0] - 6 * opp_max_cells;

        // Tamanho: ser maior vence os head-to-head.
        let diff = if opp_max_len == 0 { 0 } else { (my_len - opp_max_len).clamp(-4, 4) };
        score += 25 * diff + 18 * my_len;

        // Armadilha: região alcançável menor que o meu corpo.
        if self_space < my_len {
            score -= 3000 + 300 * (my_len - self_space);
        }

        // Fome.
        if me.health < 35 {
            score -= (35 - me.health) * 20;
        }
        let pull = 2 + if me.health < 60 { (60 - me.health) / 4 } else { 0 } + if diff <= 0 { 2 } else { 0 };
        match nearest_food {
            Some(d) => score -= pull * d.min(25),
            None if has_food => score -= 150 + (100 - me.health) * 5,
            None => {}
        }
        score += me.health / 5;

        // Hazard na cabeça.
        score -= 4 * HAZARD_DAMAGE * geo.hazard[me.body[0]];

        // Resultado terminal sempre supera qualquer avaliação heurística.
        score.clamp(LOSS / 2, WIN / 2)
    }
}

/// Roda a busca dentro do orçamento de tempo. None = não foi possível montar
/// o estado (aí o get_move cai na lógica antiga abaixo).
fn search_best_move(state: &GameState) -> Option<&'static str> {
    let geo = Geo::new(state);
    let sim = build_sim(state, &geo)?;

    let timeout = state.game.timeout as i64;
    let timeout = if timeout <= 0 { 500 } else { timeout };
    let mut budget_ms = (timeout * BUDGET_PERCENT / 100).max(30) as u64;
    if cfg!(test) {
        budget_ms = 15; // testes rápidos
    }

    let started = Instant::now();
    let mut search = Search::new(&geo, started + Duration::from_millis(budget_ms), sim.snakes.len());
    let mv = search.run(&sim)?;
    info!(
        "BUSCA turno {}: {} (profundidade {}, {} nós, {} ms)",
        state.turn,
        DIRECTIONS[mv].0,
        search.completed_depth,
        search.nodes,
        started.elapsed().as_millis()
    );
    Some(DIRECTIONS[mv].0)
}

/// POST /move — chamado a cada turno. Aqui mora a inteligência da sua cobra.
/// Precisa devolver "up", "down", "left" ou "right".
/// Exemplo do JSON recebido: https://docs.battlesnake.com/api/example-move
pub fn get_move(state: &GameState) -> Value {
    let mut is_move_safe: HashMap<&str, bool> = HashMap::from([
        ("up", true),
        ("down", true),
        ("left", true),
        ("right", true),
    ]);

    // --- Impedir que a cobra ande para trás (já implementado) ---
    // O pescoço é a parte do corpo logo atrás da cabeça. Voltar por cima dele
    // é morte certa, então marcamos aquela direção como insegura.
    let my_head = &state.you.body[0];

    // Acesso seguro ao pescoço — a cobra pode ter apenas 1 segmento no início.
    if let Some(my_neck) = state.you.body.get(1) {
        if my_neck.x < my_head.x {
            // pescoço à esquerda da cabeça -> não vá para a esquerda
            is_move_safe.insert("left", false);
        } else if my_neck.x > my_head.x {
            // pescoço à direita da cabeça -> não vá para a direita
            is_move_safe.insert("right", false);
        } else if my_neck.y < my_head.y {
            // pescoço abaixo da cabeça -> não desça
            is_move_safe.insert("down", false);
        } else if my_neck.y > my_head.y {
            // pescoço acima da cabeça -> não suba
            is_move_safe.insert("up", false);
        }
    }

    // 2. Impedir que a cobra saia do tabuleiro (paredes)
    let board_width = state.board.width;
    let board_height = state.board.height;

    if my_head.x + 1 >= board_width {
        is_move_safe.insert("right", false);
    }
    if my_head.x - 1 < 0 {
        is_move_safe.insert("left", false);
    }
    if my_head.y + 1 >= board_height {
        is_move_safe.insert("up", false);
    }
    if my_head.y - 1 < 0 {
        is_move_safe.insert("down", false);
    }

    // 3. Impedir que a cobra bata no próprio corpo
    let my_body = &state.you.body;
    for segment in my_body {
        if segment.x == my_head.x + 1 && segment.y == my_head.y {
            is_move_safe.insert("right", false);
        }
        if segment.x == my_head.x - 1 && segment.y == my_head.y {
            is_move_safe.insert("left", false);
        }
        if segment.x == my_head.x && segment.y == my_head.y + 1 {
            is_move_safe.insert("up", false);
        }
        if segment.x == my_head.x && segment.y == my_head.y - 1 {
            is_move_safe.insert("down", false);
        }
    }

    // TODO: Passo 3 — impedir que a cobra bata nas adversárias
    // let opponents = &state.board.snakes;

    // [Passo 3 implementado] Não bater no corpo das adversárias.
    let opponents = &state.board.snakes;
    for opponent in opponents {
        if opponent.id == state.you.id {
            continue; // o meu corpo já foi tratado acima
        }
        for segment in &opponent.body {
            if segment.x == my_head.x + 1 && segment.y == my_head.y {
                is_move_safe.insert("right", false);
            }
            if segment.x == my_head.x - 1 && segment.y == my_head.y {
                is_move_safe.insert("left", false);
            }
            if segment.x == my_head.x && segment.y == my_head.y + 1 {
                is_move_safe.insert("up", false);
            }
            if segment.x == my_head.x && segment.y == my_head.y - 1 {
                is_move_safe.insert("down", false);
            }
        }
    }

    // [Busca com lookahead] Primeira escolha: olha vários turnos à frente.
    // Se não conseguir montar o estado, segue para a lógica anterior abaixo.
    if let Some(best_move) = search_best_move(state) {
        return json!({ "move": best_move });
    }

    // Sobrou alguma direção segura?
    let safe_moves: Vec<&str> = is_move_safe
        .into_iter()
        .filter(|(_, is_safe)| *is_safe)
        .map(|(direction, _)| direction)
        .collect();

    if safe_moves.is_empty() {
        // Emergência: todas as direções são perigosas.
        // Escolhemos uma ao acaso entre as 4 — melhor do que uma direção fixa.
        let all_moves = ["up", "down", "left", "right"];
        let fallback = all_moves
            .choose(&mut rand::rng())
            .expect("array não está vazio");
        info!("MOVE {}: sem saída! emergência -> {}", state.turn, fallback);
        return json!({ "move": fallback });
    }

    // Escolhe uma direção segura ao acaso.
    let chosen = safe_moves
        .choose(&mut rand::rng())
        .expect("safe_moves não está vazio");

    // TODO: Passo 4 — ir atrás da comida em vez de sortear, para não morrer de fome
    // let food = &state.board.food;

    // [Passo 4 implementado + espaço + head-to-head]
    // Entre as direções seguras, dá uma nota para cada uma e escolhe a melhor.
    // O sorteio acima fica só como reserva caso nenhuma direção seja pontuada.
    let food = &state.board.food;
    let blocked = build_blocked(state);
    let danger = build_danger(state);
    let my_len = state.you.body.len();
    let health = state.you.health;

    let mut best: Option<(&str, i64)> = None;

    for (name, dx, dy) in DIRECTIONS {
        if !safe_moves.contains(&name) {
            continue;
        }
        let (nx, ny) = (my_head.x + dx, my_head.y + dy);
        if !in_bounds(nx, ny, board_width, board_height) {
            continue;
        }

        let mut score: i64 = 0;

        // Espaço: prefira lados com muito espaço e fuja de becos.
        let area = flood_fill((nx, ny), &blocked, board_width, board_height, my_len * 2);
        score += area as i64 * 10;
        if area < my_len {
            score -= 1000; // beco sem saída: quase morte certa
        }

        // Adversárias: evite a casa onde uma cobra maior ou igual pode chegar.
        if danger[nx as usize][ny as usize] {
            score -= 500;
        }

        // Comida: quanto mais perto, melhor. Com pouca vida a fome pesa mais.
        let nearest_food = food
            .iter()
            .map(|f| (f.x - nx).abs() + (f.y - ny).abs())
            .min();
        if let Some(dist) = nearest_food {
            let weight = if health < 40 { 20 } else { 3 };
            score -= dist as i64 * weight;
        }

        if best.map_or(true, |(_, s)| score > s) {
            best = Some((name, score));
        }
    }

    if let Some((best_move, score)) = best {
        info!("MOVE {}: {} (score {})", state.turn, best_move, score);
        return json!({ "move": best_move });
    }

    info!("MOVE {}: {}", state.turn, chosen);
    json!({ "move": chosen })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Battlesnake, Board, Coord, Game};

    /// Monta um estado de jogo mínimo para os testes, com a cobra deitada
    /// na horizontal: cabeça em `head` e pescoço em `neck`.
    fn game_state(head: Coord, neck: Coord) -> GameState {
        let you = Battlesnake {
            id: "minha-cobra".to_string(),
            name: "MinhaCobra".to_string(),
            health: 100,
            body: vec![head, neck, Coord { x: neck.x, y: neck.y - 1 }],
            head,
            length: 3,
            latency: Some("50".to_string()),
            shout: None,
        };

        GameState {
            game: Game {
                id: "partida-de-teste".to_string(),
                ruleset: HashMap::new(),
                map: Some("standard".to_string()),
                timeout: 500,
            },
            turn: 4,
            board: Board {
                height: 11,
                width: 11,
                food: vec![Coord { x: 5, y: 5 }],
                hazards: vec![],
                snakes: vec![you.clone()],
            },
            you,
        }
    }

    fn chosen_move(state: &GameState) -> String {
        get_move(state)["move"].as_str().unwrap().to_string()
    }

    #[test]
    fn info_devolve_os_campos_obrigatorios() {
        let response = info();

        assert_eq!(response["apiversion"], "1");
        assert!(response.get("author").is_some());
        assert!(response.get("color").is_some());
        assert!(response.get("head").is_some());
        assert!(response.get("tail").is_some());
    }

    #[test]
    fn move_devolve_sempre_uma_direcao_valida() {
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });

        for _ in 0..50 {
            let direction = chosen_move(&state);
            assert!(
                ["up", "down", "left", "right"].contains(&direction.as_str()),
                "direção inválida: {direction}"
            );
        }
    }

    #[test]
    fn nunca_volta_por_cima_do_pescoco() {
        // pescoço à esquerda da cabeça: "left" seria andar para trás
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        for _ in 0..50 {
            assert_ne!(chosen_move(&state), "left");
        }

        // pescoço à direita da cabeça: "right" seria andar para trás
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 6, y: 4 });
        for _ in 0..50 {
            assert_ne!(chosen_move(&state), "right");
        }

        // pescoço abaixo da cabeça: "down" seria andar para trás
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 5, y: 3 });
        for _ in 0..50 {
            assert_ne!(chosen_move(&state), "down");
        }

        // pescoço acima da cabeça: "up" seria andar para trás
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 5, y: 5 });
        for _ in 0..50 {
            assert_ne!(chosen_move(&state), "up");
        }
    }

    #[test]
    fn evita_parede_quando_tem_opcao() {
        // Cobra no canto inferior esquerdo, pescoço à direita da cabeça:
        // não pode ir para right (pescoço) nem left (x=-1) nem down (y=-1).
        // A única opção segura é "up".
        let state = game_state(Coord { x: 0, y: 0 }, Coord { x: 1, y: 0 });
        for _ in 0..50 {
            let direction = chosen_move(&state);
            assert!(
                ["up", "down", "left", "right"].contains(&direction.as_str()),
                "direção inválida: {direction}"
            );
            assert_ne!(direction, "left",  "foi para fora do tabuleiro (esquerda)");
            assert_ne!(direction, "down",  "foi para fora do tabuleiro (baixo)");
        }
    }

    #[test]
    fn evita_proprio_corpo_quando_tem_opcao() {
        // Cabeça em (5,4), pescoço à esquerda (4,4), corpo acima em (5,5).
        // Restam right e down. Verificamos que nunca escolhe "left" nem "up".
        let head = Coord { x: 5, y: 4 };
        let neck = Coord { x: 4, y: 4 };
        let mut state = game_state(head, neck);
        state.you.body = vec![head, neck, Coord { x: 5, y: 5 }, Coord { x: 4, y: 3 }];
        state.board.snakes = vec![state.you.clone()];

        for _ in 0..50 {
            let direction = chosen_move(&state);
            assert_ne!(direction, "left", "voltou pelo pescoço");
            assert_ne!(direction, "up", "bateu no próprio corpo");
            assert!(["right", "down"].contains(&direction.as_str()));
        }
    }

    #[test]
    fn comportamento_definido_sem_safe_moves() {
        // Cabeça no canto (0,0), pescoço acima (0,1) — bloqueia up.
        // left (x=-1) e down (y=-1) saem do tabuleiro.
        // Somente right estaria livre, mas o helper adiciona um segmento
        // em (1,0) para fechar todas as saídas e testar o fallback.
        //
        // Independentemente de qual direção for escolhida, não pode lançar
        // pânico e deve ser uma das quatro direções válidas.
        let head = Coord { x: 0, y: 0 };
        let neck = Coord { x: 0, y: 1 };
        let mut state = game_state(head, neck);
        // Adiciona um segmento do corpo à direita para bloquear "right"
        state.you.body.push(Coord { x: 1, y: 0 });
        state.board.snakes = vec![state.you.clone()];

        let direction = chosen_move(&state);
        assert!(
            ["up", "down", "left", "right"].contains(&direction.as_str()),
            "fallback retornou direção inválida: {direction}"
        );
    }

    // ----- Testes novos (lógica avançada) -----

    fn enemy(id: &str, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health: 90,
            head: body[0],
            length: body.len() as i32,
            body,
            latency: None,
            shout: None,
        }
    }

    #[test]
    fn vai_em_direcao_a_comida() {
        // cabeça (5,4), pescoço à esquerda, comida em (5,5) logo acima
        let mut state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        state.you.health = 20; // com fome, a comida pesa mais que o território
        state.board.snakes = vec![
            state.you.clone(),
            enemy("longe", vec![Coord { x: 10, y: 10 }, Coord { x: 10, y: 9 }, Coord { x: 10, y: 8 }]),
        ];
        assert_eq!(chosen_move(&state), "up");
    }

    #[test]
    fn nao_bate_no_corpo_da_adversaria() {
        let mut state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        state.board.food = vec![Coord { x: 9, y: 4 }]; // comida à direita
        // corpo da adversária bloqueando (6,4), logo à direita
        state.board.snakes.push(enemy(
            "inimiga",
            vec![Coord { x: 6, y: 6 }, Coord { x: 6, y: 5 }, Coord { x: 6, y: 4 }, Coord { x: 6, y: 3 }],
        ));
        for _ in 0..30 {
            assert_ne!(chosen_move(&state), "right");
        }
    }

    #[test]
    fn evita_cabeca_de_adversaria_maior() {
        let mut state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        state.board.food = vec![];
        // adversária maior com a cabeça em (7,4): a casa (6,4) é perigosa
        state.board.snakes.push(enemy(
            "inimiga",
            vec![Coord { x: 7, y: 4 }, Coord { x: 8, y: 4 }, Coord { x: 9, y: 4 }, Coord { x: 9, y: 5 }],
        ));
        for _ in 0..30 {
            assert_ne!(chosen_move(&state), "right");
        }
    }

    #[test]
    fn foge_de_beco_sem_saida() {
        // "up" leva a uma casinha cercada (1 casa livre); a comida está lá.
        let head = Coord { x: 5, y: 5 };
        let mut state = game_state(head, Coord { x: 4, y: 5 });
        state.board.food = vec![Coord { x: 5, y: 6 }];
        state.you.body = vec![
            head, Coord { x: 4, y: 5 }, Coord { x: 4, y: 6 }, Coord { x: 4, y: 7 },
            Coord { x: 5, y: 7 }, Coord { x: 6, y: 7 }, Coord { x: 6, y: 6 }, Coord { x: 7, y: 6 },
        ];
        state.you.health = 80;
        state.board.snakes = vec![state.you.clone()];
        for _ in 0..20 {
            assert_ne!(chosen_move(&state), "up");
        }
    }
}