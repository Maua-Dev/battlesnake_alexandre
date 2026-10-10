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

use crate::models::GameState;
use rand::seq::IndexedRandom;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::collections::VecDeque;
use tracing::info;

/// GET / — chamado quando você cadastra a cobra no site e a cada partida.
/// Controla a aparência dela. Opções de cabeça, cauda e cor:
/// https://docs.battlesnake.com/guides/customizations
pub fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "xandealee",          // TODO: coloque aqui o SEU usuário do Battlesnake
        "color": "#5a008b",    // TODO: escolha a cor da sua cobra
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
            body: vec![
                head,
                neck,
                Coord {
                    x: neck.x,
                    y: neck.y - 1,
                },
            ],
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
            assert_ne!(direction, "left", "foi para fora do tabuleiro (esquerda)");
            assert_ne!(direction, "down", "foi para fora do tabuleiro (baixo)");
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
        let state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        assert_eq!(chosen_move(&state), "up");
    }

    #[test]
    fn nao_bate_no_corpo_da_adversaria() {
        let mut state = game_state(Coord { x: 5, y: 4 }, Coord { x: 4, y: 4 });
        state.board.food = vec![Coord { x: 9, y: 4 }]; // comida à direita
                                                       // corpo da adversária bloqueando (6,4), logo à direita
        state.board.snakes.push(enemy(
            "inimiga",
            vec![
                Coord { x: 6, y: 6 },
                Coord { x: 6, y: 5 },
                Coord { x: 6, y: 4 },
                Coord { x: 6, y: 3 },
            ],
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
            vec![
                Coord { x: 7, y: 4 },
                Coord { x: 8, y: 4 },
                Coord { x: 9, y: 4 },
                Coord { x: 9, y: 5 },
            ],
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
            head,
            Coord { x: 4, y: 5 },
            Coord { x: 4, y: 6 },
            Coord { x: 4, y: 7 },
            Coord { x: 5, y: 7 },
            Coord { x: 6, y: 7 },
            Coord { x: 6, y: 6 },
            Coord { x: 7, y: 6 },
        ];
        state.you.health = 80;
        state.board.snakes = vec![state.you.clone()];
        for _ in 0..20 {
            assert_ne!(chosen_move(&state), "up");
        }
    }
}
